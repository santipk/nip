//! Durable local chat. Disk work runs outside the QUIC runtime and messages are saved before broadcast.
use crate::{
    contracts::{Channel, ChannelKind, ChatMessage},
    storage::{protect, unprotect},
};
use rusqlite::{Connection, params};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub struct History(Arc<Mutex<Connection>>);
impl History {
    pub async fn open(path: PathBuf) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(path.parent().ok_or("Historial sin carpeta")?).map_err(|e| format!("No se pudo crear la carpeta del servidor: {e}"))?;
            let connection = Connection::open(path).map_err(|e| format!("No se pudo abrir el historial: {e}"))?;
            connection.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
            connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS messages (id INTEGER PRIMARY KEY, sender_id INTEGER NOT NULL, payload BLOB NOT NULL);").map_err(|e| format!("No se pudo preparar el historial: {e}"))?;
            let has_channel = {
                let mut columns = connection.prepare("PRAGMA table_info(messages)").map_err(|e| e.to_string())?;
                let names = columns.query_map([], |row| row.get::<_, String>(1)).map_err(|e| e.to_string())?;
                names.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?.iter().any(|name| name == "channel_id")
            };
            if !has_channel {
                connection.execute_batch("ALTER TABLE messages ADD COLUMN channel_id TEXT NOT NULL DEFAULT 'general';").map_err(|e| format!("No se pudo migrar el historial: {e}"))?;
            }
            connection.execute_batch("CREATE INDEX IF NOT EXISTS messages_channel_id ON messages(channel_id,id);").map_err(|e| e.to_string())?;
            connection.execute_batch("CREATE TABLE IF NOT EXISTS channels (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('text','voice'))); CREATE TABLE IF NOT EXISTS counters (key TEXT PRIMARY KEY, value INTEGER NOT NULL);").map_err(|e| e.to_string())?;
            let count: i64 = connection.query_row("SELECT COUNT(*) FROM channels", [], |row| row.get(0)).map_err(|e| e.to_string())?;
            if count == 0 {
                connection.execute_batch("INSERT INTO channels VALUES ('general','general','text'),('voice-general','General','voice');").map_err(|e| e.to_string())?;
            }
            Ok(Self(Arc::new(Mutex::new(connection))))
        }).await.map_err(|_| "Se interrumpió la apertura del historial".to_string())?
    }
    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let database = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = database
                .lock()
                .map_err(|_| "El historial no está disponible")?;
            f(&mut connection)
        })
        .await
        .map_err(|_| "Se interrumpió la operación del historial".to_string())?
    }
    pub async fn counters(&self) -> Result<(u64, u64), String> {
        self.run(|connection| {
            let (message, peer): (i64, i64) = connection
                .query_row(
                    "SELECT MAX(COALESCE(MAX(id),0),COALESCE((SELECT value FROM counters WHERE key='message'),0))+1, MAX(COALESCE(MAX(sender_id),0),COALESCE((SELECT value FROM counters WHERE key='peer'),0))+1 FROM messages",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| e.to_string())?;
            Ok((
                u64::try_from(message).map_err(|_| "Contador de mensajes inválido")?,
                u64::try_from(peer).map_err(|_| "Contador de participantes inválido")?,
            ))
        })
        .await
    }
    pub async fn snapshot(&self, path: PathBuf) -> Result<(), String> {
        if path.exists() {
            return Err("El destino del snapshot ya existe".into());
        }
        self.run(move |connection| {
            let mut target = Connection::open(path).map_err(|e| e.to_string())?;
            let backup = rusqlite::backup::Backup::new(connection, &mut target)
                .map_err(|e| e.to_string())?;
            backup
                .run_to_completion(128, Duration::from_millis(5), None)
                .map_err(|e| format!("No se pudo respaldar SQLite: {e}"))
        })
        .await
    }
    pub async fn channels(&self) -> Result<Vec<Channel>, String> {
        self.run(|connection| {
            let mut query = connection
                .prepare("SELECT id,name,kind FROM channels ORDER BY rowid")
                .map_err(|e| e.to_string())?;
            query
                .query_map([], |row| {
                    Ok(Channel {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        kind: if row.get::<_, String>(2)? == "text" {
                            ChannelKind::Text
                        } else {
                            ChannelKind::Voice
                        },
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn create_channel(&self, name: String, kind: ChannelKind) -> Result<Channel, String> {
        let name = valid_channel_name(name)?;
        let channel = Channel {
            id: format!(
                "{:016x}{:016x}",
                getrandom::u64().map_err(|e| e.to_string())?,
                getrandom::u64().map_err(|e| e.to_string())?
            ),
            name,
            kind,
        };
        let created = channel.clone();
        self.run(move |connection| {
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM channels", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            if count >= 128 {
                return Err("Máximo 128 canales por servidor".into());
            }
            connection
                .execute(
                    "INSERT INTO channels(id,name,kind) VALUES (?1,?2,?3)",
                    params![
                        channel.id,
                        channel.name,
                        if kind == ChannelKind::Text {
                            "text"
                        } else {
                            "voice"
                        }
                    ],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .await?;
        Ok(created)
    }
    pub async fn rename_channel(&self, id: String, name: String) -> Result<(), String> {
        let name = valid_channel_name(name)?;
        self.run(move |connection| {
            let changed = connection
                .execute("UPDATE channels SET name=?1 WHERE id=?2", params![name, id])
                .map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err("El canal no existe".into());
            }
            Ok(())
        })
        .await
    }
    pub async fn delete_channel(&self, id: String) -> Result<(), String> {
        self.run(move |connection| {
            let tx = connection.transaction().map_err(|e| e.to_string())?;
            let kind: String = tx.query_row("SELECT kind FROM channels WHERE id=?1", [&id], |row| row.get(0)).map_err(|_| "El canal no existe")?;
            if kind == "text" {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM channels WHERE kind='text'", [], |row| row.get(0)).map_err(|e| e.to_string())?;
                if count <= 1 { return Err("Debe quedar al menos un canal de texto".into()); }
            }
            tx.execute_batch("INSERT INTO counters(key,value) VALUES ('message',(SELECT COALESCE(MAX(id),0) FROM messages)),('peer',(SELECT COALESCE(MAX(sender_id),0) FROM messages)) ON CONFLICT(key) DO UPDATE SET value=MAX(value,excluded.value);").map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM messages WHERE channel_id=?1", [&id]).map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM channels WHERE id=?1", [&id]).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())
        }).await
    }
    pub async fn append(&self, message: ChatMessage) -> Result<(), String> {
        self.append_to(crate::contracts::GENERAL_TEXT_CHANNEL.into(), message)
            .await
    }
    pub async fn ensure_message(&self, message: ChatMessage) -> Result<(), String> {
        self.run(move |connection| {
            use rusqlite::OptionalExtension;
            let existing: Option<Vec<u8>> = connection
                .query_row(
                    "SELECT payload FROM messages WHERE id=?1",
                    [message.id as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(payload) = existing {
                let existing: ChatMessage = serde_json::from_slice(&unprotect(&payload)?)
                    .map_err(|_| "Mensaje confirmado dañado")?;
                if existing != message {
                    return Err("El identificador ya pertenece a otro mensaje".into());
                }
                return Ok(());
            }
            let payload = protect(&serde_json::to_vec(&message).map_err(|e| e.to_string())?)?;
            connection
                .execute(
                    "INSERT INTO messages(id,sender_id,payload,channel_id) VALUES (?1,?2,?3,?4)",
                    params![
                        i64::try_from(message.id).map_err(|_| "ID inválido")?,
                        i64::try_from(message.sender_id).map_err(|_| "ID inválido")?,
                        payload,
                        message.channel_id
                    ],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
    }
    pub async fn append_to(
        &self,
        channel_id: String,
        mut message: ChatMessage,
    ) -> Result<(), String> {
        message.channel_id = channel_id.clone();
        self.run(move |connection| {
            let data = protect(&serde_json::to_vec(&message).map_err(|e| e.to_string())?)?;
            connection
                .execute(
                    "INSERT INTO messages (id,sender_id,payload,channel_id) VALUES (?1,?2,?3,?4)",
                    params![
                        i64::try_from(message.id).map_err(|_| "ID inválido")?,
                        i64::try_from(message.sender_id)
                            .map_err(|_| "ID de participante inválido")?,
                        data,
                        channel_id
                    ],
                )
                .map_err(|e| format!("No se pudo guardar el mensaje; no se envió al grupo: {e}"))?;
            Ok(())
        })
        .await
    }
    pub async fn seed(&self, messages: Vec<ChatMessage>) -> Result<(), String> {
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(|e| e.to_string())?;
            for message in messages {
                let data = protect(&serde_json::to_vec(&message).map_err(|e| e.to_string())?)?;
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO messages (id,sender_id,payload,channel_id) VALUES (?1,?2,?3,?4)",
                        params![
                            i64::try_from(message.id).map_err(|_| "ID inválido")?,
                            i64::try_from(message.sender_id)
                                .map_err(|_| "ID de participante inválido")?,
                            data,
                            message.channel_id
                        ],
                    )
                    .map_err(|e| e.to_string())?;
            }
            transaction.commit().map_err(|e| e.to_string())
        })
        .await
    }
    pub async fn page(&self, before: u64) -> Result<(Vec<ChatMessage>, bool), String> {
        self.page_in(crate::contracts::GENERAL_TEXT_CHANNEL.into(), before)
            .await
    }
    pub async fn page_in(
        &self,
        channel_id: String,
        before: u64,
    ) -> Result<(Vec<ChatMessage>, bool), String> {
        self.run(move |connection| {
            let mut query = connection
                .prepare("SELECT payload FROM messages WHERE id < ?1 AND channel_id = ?2 ORDER BY id DESC LIMIT 101")
                .map_err(|e| e.to_string())?;
            let rows = query
                .query_map(params![before.min(i64::MAX as u64) as i64, channel_id], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .map_err(|e| e.to_string())?;
            let mut messages = Vec::new();
            for row in rows {
                messages.push(
                    serde_json::from_slice(&unprotect(&row.map_err(|e| e.to_string())?)?)
                        .map_err(|_| "El historial contiene un mensaje dañado")?,
                );
            }
            let more = messages.len() > 100;
            messages.truncate(100);
            messages.reverse();
            Ok((messages, more))
        })
        .await
    }
}

fn valid_channel_name(name: String) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("El nombre del canal debe tener entre 1 y 64 caracteres sin controles".into());
    }
    Ok(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn previous_schema_and_protected_payload_migrate_without_changing_history() {
        let directory =
            std::env::temp_dir().join(format!("nip-legacy-history-{}", getrandom::u64().unwrap()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("history.sqlite3");
        let database = Connection::open(&path).unwrap();
        database.execute_batch("CREATE TABLE messages(id INTEGER PRIMARY KEY,sender_id INTEGER NOT NULL,payload BLOB NOT NULL);").unwrap();
        let payload = protect(br#"{"id":7,"sender_id":4,"sender_name":"Anterior","text":"Conservar historial","sent_at":123}"#).unwrap();
        database
            .execute("INSERT INTO messages VALUES (7,4,?1)", [payload])
            .unwrap();
        drop(database);
        let history = History::open(path).await.unwrap();
        let messages = history.page(u64::MAX).await.unwrap().0;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "Conservar historial");
        assert_eq!(messages[0].channel_id, "general");
        assert!(messages[0].attachments.is_empty());
        assert_eq!(
            history.channels().await.unwrap(),
            crate::contracts::default_channels()
        );
        assert_eq!(history.counters().await.unwrap(), (8, 5));
        drop(history);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn failed_write_is_reported_and_does_not_add_history() {
        let directory =
            std::env::temp_dir().join(format!("nip-history-failure-{}", getrandom::u64().unwrap()));
        let history = History::open(directory.join("history.sqlite3"))
            .await
            .unwrap();
        history
            .run(|connection| {
                connection
                    .execute_batch("PRAGMA query_only=ON")
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap();
        let result = history
            .append(ChatMessage {
                attachments: vec![],
                channel_id: "general".into(),
                id: 1,
                sender_id: 1,
                sender_name: "QA".into(),
                text: "No confirmar".into(),
                sent_at: 1,
            })
            .await;
        assert!(result.is_err());
        assert!(history.page(u64::MAX).await.unwrap().0.is_empty());
        drop(history);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn history_is_durable_paginated_and_payloads_are_protected() {
        let directory = std::env::temp_dir().join(format!(
            "nip-history-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        let path = directory.join("history.sqlite3");
        let history = History::open(path.clone()).await.unwrap();
        let messages: Vec<_> = (1..=205)
            .map(|id| ChatMessage {
                attachments: vec![],
                channel_id: "general".into(),
                id,
                sender_id: 7,
                sender_name: "Autor".into(),
                text: format!("mensaje privado {id}"),
                sent_at: id,
            })
            .collect();
        history.seed(messages).await.unwrap();
        drop(history);
        let history = History::open(path.clone()).await.unwrap();
        assert_eq!(history.counters().await.unwrap(), (206, 8));
        let (recent, more) = history.page(u64::MAX).await.unwrap();
        assert_eq!(recent.len(), 100);
        assert_eq!(recent[0].id, 106);
        assert!(more);
        let (older, more) = history.page(106).await.unwrap();
        assert_eq!(older[0].id, 6);
        assert_eq!(older.last().unwrap().id, 105);
        assert!(more);
        let (oldest, more) = history.page(6).await.unwrap();
        assert_eq!(oldest.len(), 5);
        assert!(!more);
        drop(history);
        #[cfg(windows)]
        assert!(
            !std::fs::read(&path)
                .unwrap()
                .windows(15)
                .any(|bytes| bytes == b"mensaje privado")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
