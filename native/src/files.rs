//! Host-owned attachments, bounded upload reservations and a durable publication outbox.
use crate::{
    contracts::{Attachment, ChatMessage},
    storage::{protect, unprotect},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: u64,
    pub max_server_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 100_000_000,
            max_server_bytes: 10_000_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub used_bytes: u64,
    pub reserved_bytes: u64,
}
struct State {
    database: Connection,
    limits: Limits,
}
#[derive(Clone)]
pub struct AttachmentStore {
    root: PathBuf,
    state: Arc<Mutex<State>>,
}
pub enum BeginUpload {
    Pending(Upload),
    Committed(Attachment),
}
pub struct Upload {
    store: AttachmentStore,
    metadata: Attachment,
    file: Option<File>,
    hasher: Sha256,
    written: u64,
    finished: bool,
}

impl AttachmentStore {
    pub fn open(server_directory: PathBuf, limits: Limits) -> Result<Self, String> {
        valid_limits(limits)?;
        let root = server_directory.join("attachments");
        std::fs::create_dir_all(root.join("pending")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(root.join("blobs")).map_err(|e| e.to_string())?;
        let database = Connection::open(root.join("index.sqlite3")).map_err(|e| e.to_string())?;
        database.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS attachments(id TEXT PRIMARY KEY,operation_id TEXT UNIQUE NOT NULL,channel_id TEXT NOT NULL,name TEXT NOT NULL,size INTEGER NOT NULL,sha256 TEXT NOT NULL,state TEXT NOT NULL CHECK(state IN ('uploading','ready','published')),publication BLOB);").map_err(|e|e.to_string())?;
        database
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS deletions(id TEXT PRIMARY KEY,size INTEGER NOT NULL);",
            )
            .map_err(|e| e.to_string())?;
        clean_deletions(&root, &database)?;
        let unfinished = {
            let mut query = database
                .prepare("SELECT id FROM attachments WHERE state='uploading'")
                .map_err(|e| e.to_string())?;
            query
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for id in unfinished {
            if !internal_id(&id) {
                return Err("Índice de archivos dañado".into());
            }
            remove_if_exists(root.join("pending").join(&id))?;
            remove_if_exists(root.join("blobs").join(&id))?;
        }
        database
            .execute("DELETE FROM attachments WHERE state='uploading'", [])
            .map_err(|e| e.to_string())?;
        // A crash can occur after create_new and before its reservation INSERT.
        for entry in std::fs::read_dir(root.join("pending")).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || !internal_id(&entry.file_name().to_string_lossy())
            {
                return Err(
                    "Temporal de adjunto desconocido; revisá la carpeta de archivos".into(),
                );
            }
            remove_if_exists(entry.path())?;
        }
        Ok(Self {
            root,
            state: Arc::new(Mutex::new(State { database, limits })),
        })
    }
    pub fn begin(
        &self,
        operation_id: String,
        channel_id: String,
        name: String,
        size: u64,
        sha256: String,
        persistent: bool,
    ) -> Result<BeginUpload, String> {
        if !persistent {
            return Err(
                "Convertí el servidor a persistente antes de adjuntar archivos durables".into(),
            );
        }
        if !(32..=64).contains(&operation_id.len())
            || !operation_id.bytes().all(|b| b.is_ascii_hexdigit())
            || !safe_channel(&channel_id)
            || name.is_empty()
            || name.chars().count() > 255
            || name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || name == "."
            || name == ".."
            || sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Metadatos del archivo inválidos".into());
        }
        let sha256 = sha256.to_ascii_lowercase();
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        if let Some((metadata,status))=state.database.query_row("SELECT id,channel_id,name,size,sha256,state FROM attachments WHERE operation_id=?1",[&operation_id],|row|Ok((attachment(row)?,row.get::<_,String>(5)?))).optional().map_err(|e|e.to_string())? {
            if metadata.channel_id!=channel_id || metadata.name!=name || metadata.size!=size || metadata.sha256!=sha256 {return Err("El identificador de envío ya corresponde a otro archivo".into());}
            return if status=="uploading" {Err("Este envío ya está en curso".into())} else {Ok(BeginUpload::Committed(metadata))};
        }
        let usage = read_usage(&state.database)?;
        if size > state.limits.max_file_bytes {
            return Err(format!(
                "El archivo supera el límite de {} bytes",
                state.limits.max_file_bytes
            ));
        }
        if size > i64::MAX as u64
            || usage
                .used_bytes
                .checked_add(usage.reserved_bytes)
                .and_then(|n| n.checked_add(size))
                .is_none_or(|n| n > state.limits.max_server_bytes)
        {
            return Err("La cuota del servidor no tiene espacio disponible para este envío".into());
        }
        let id = format!(
            "{:016x}{:016x}",
            getrandom::u64().map_err(|e| e.to_string())?,
            getrandom::u64().map_err(|e| e.to_string())?
        );
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.root.join("pending").join(&id))
            .map_err(|e| format!("No se pudo reservar el archivo temporal: {e}"))?;
        if let Err(error)=state.database.execute("INSERT INTO attachments(id,operation_id,channel_id,name,size,sha256,state) VALUES (?1,?2,?3,?4,?5,?6,'uploading')",params![id,operation_id,channel_id,name,size as i64,sha256]) {drop(file);let _=std::fs::remove_file(self.root.join("pending").join(&id));return Err(error.to_string());}
        Ok(BeginUpload::Pending(Upload {
            store: self.clone(),
            metadata: Attachment {
                id,
                channel_id,
                name,
                size,
                sha256,
            },
            file: Some(file),
            hasher: Sha256::new(),
            written: 0,
            finished: false,
        }))
    }
    pub fn usage(&self) -> Result<Usage, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        read_usage(&state.database)
    }
    pub fn limits(&self) -> Result<Limits, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?
            .limits)
    }
    pub fn set_limits(&self, limits: Limits) -> Result<(), String> {
        valid_limits(limits)?;
        self.state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?
            .limits = limits;
        Ok(())
    }
    pub fn publication(&self, operation_id: &str) -> Result<Option<ChatMessage>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let bytes: Option<Vec<u8>> = state
            .database
            .query_row(
                "SELECT publication FROM attachments WHERE operation_id=?1 AND state!='uploading'",
                [operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        bytes
            .map(|bytes| {
                serde_json::from_slice(&unprotect(&bytes)?)
                    .map_err(|_| "Publicación de archivo dañada".into())
            })
            .transpose()
    }
    pub fn pending_publications(&self) -> Result<Vec<ChatMessage>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let mut query = state
            .database
            .prepare("SELECT publication FROM attachments WHERE state='ready' ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            serde_json::from_slice(&unprotect(&row.map_err(|e| e.to_string())?)?)
                .map_err(|_| "Publicación de archivo dañada".into())
        })
        .collect()
    }
    pub fn mark_published(&self, id: &str) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let changed = state
            .database
            .execute(
                "UPDATE attachments SET state='published' WHERE id=?1 AND state!='uploading'",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("El archivo no está confirmado".into());
        }
        Ok(())
    }
    pub fn open_attachment(
        &self,
        channel_id: &str,
        id: &str,
    ) -> Result<(Attachment, File), String> {
        if !internal_id(id) {
            return Err("Identificador de archivo inválido".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let metadata=state.database.query_row("SELECT id,channel_id,name,size,sha256 FROM attachments WHERE id=?1 AND channel_id=?2 AND state='published'",params![id,channel_id],attachment).optional().map_err(|e|e.to_string())?.ok_or("El archivo no existe en este canal o todavía no se publicó")?;
        let file = File::open(self.root.join("blobs").join(id))
            .map_err(|e| format!("No se pudo abrir el adjunto: {e}"))?;
        if file.metadata().map_err(|e| e.to_string())?.len() != metadata.size {
            return Err("El archivo alojado no coincide con su longitud".into());
        }
        Ok((metadata, file))
    }
    pub fn delete_channel(&self, channel_id: &str) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let transaction = state
            .database
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        transaction.execute("INSERT OR IGNORE INTO deletions SELECT id,size FROM attachments WHERE channel_id=?1",[channel_id]).map_err(|e|e.to_string())?;
        transaction
            .execute("DELETE FROM attachments WHERE channel_id=?1", [channel_id])
            .map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())?;
        clean_deletions(&self.root, &state.database)
    }
    pub fn remove_missing_channels(&self, valid_ids: &[String]) -> Result<(), String> {
        let channels = {
            let state = self
                .state
                .lock()
                .map_err(|_| "Almacenamiento no disponible")?;
            let mut query = state
                .database
                .prepare("SELECT DISTINCT channel_id FROM attachments")
                .map_err(|e| e.to_string())?;
            query
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for channel in channels {
            if !valid_ids.contains(&channel) {
                self.delete_channel(&channel)?;
            }
        }
        Ok(())
    }
    fn cancel(&self, id: &str) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        let status: Option<String> = state
            .database
            .query_row("SELECT state FROM attachments WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if status.as_deref() == Some("uploading") {
            remove_if_exists(self.root.join("pending").join(id))?;
            remove_if_exists(self.root.join("blobs").join(id))?;
            state
                .database
                .execute(
                    "DELETE FROM attachments WHERE id=?1 AND state='uploading'",
                    [id],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl Upload {
    pub fn metadata(&self) -> &Attachment {
        &self.metadata
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self
            .written
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.metadata.size)
        {
            return Err("La transferencia supera la longitud declarada".into());
        }
        self.file
            .as_mut()
            .ok_or("El envío ya terminó")?
            .write_all(bytes)
            .map_err(|e| format!("No se pudo guardar el archivo: {e}"))?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }
    pub fn commit(mut self, mut message: ChatMessage) -> Result<Attachment, String> {
        if self.written != self.metadata.size
            || format!("{:x}", self.hasher.clone().finalize()) != self.metadata.sha256
        {
            return Err("El archivo está incompleto o no coincide con su hash SHA-256".into());
        }
        if message.channel_id != self.metadata.channel_id
            || message.id == 0
            || message.id >= i64::MAX as u64
            || message.sender_id == 0
            || message.sender_id >= i64::MAX as u64
            || message.sender_name.trim().is_empty()
            || message.sender_name.chars().count() > 64
            || message.text.chars().count() > 2000
        {
            return Err("La publicación no corresponde al canal del adjunto".into());
        }
        message.attachments = vec![self.metadata.clone()];
        let publication = protect(&serde_json::to_vec(&message).map_err(|e| e.to_string())?)?;
        let file = self.file.take().ok_or("El envío ya terminó")?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let state = self
            .store
            .state
            .lock()
            .map_err(|_| "Almacenamiento no disponible")?;
        std::fs::rename(
            self.store.root.join("pending").join(&self.metadata.id),
            self.store.root.join("blobs").join(&self.metadata.id),
        )
        .map_err(|e| format!("No se pudo confirmar el archivo: {e}"))?;
        let changed=state.database.execute("UPDATE attachments SET state='ready',publication=?1 WHERE id=?2 AND state='uploading'",params![publication,self.metadata.id]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("El envío fue cancelado antes de confirmarse".into());
        }
        self.finished = true;
        Ok(self.metadata.clone())
    }
    pub fn cancel(mut self) -> Result<(), String> {
        self.file.take();
        self.store.cancel(&self.metadata.id)?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for Upload {
    fn drop(&mut self) {
        if !self.finished {
            self.file.take();
            let _ = self.store.cancel(&self.metadata.id);
        }
    }
}
fn attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        name: row.get(2)?,
        size: row.get::<_, i64>(3)? as u64,
        sha256: row.get(4)?,
    })
}
fn read_usage(database: &Connection) -> Result<Usage, String> {
    database.query_row("SELECT COALESCE(SUM(CASE WHEN state!='uploading' THEN size ELSE 0 END),0)+COALESCE((SELECT SUM(size) FROM deletions),0),COALESCE(SUM(CASE WHEN state='uploading' THEN size ELSE 0 END),0) FROM attachments",[],|row|Ok(Usage{used_bytes:row.get::<_,i64>(0)? as u64,reserved_bytes:row.get::<_,i64>(1)? as u64})).map_err(|e|e.to_string())
}
fn clean_deletions(root: &std::path::Path, database: &Connection) -> Result<(), String> {
    let ids = {
        let mut query = database
            .prepare("SELECT id FROM deletions")
            .map_err(|e| e.to_string())?;
        query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    for id in ids {
        if !internal_id(&id) {
            return Err("Índice de eliminación de adjuntos dañado".into());
        }
        remove_if_exists(root.join("pending").join(&id))?;
        remove_if_exists(root.join("blobs").join(&id))?;
        database
            .execute("DELETE FROM deletions WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn valid_limits(limits: Limits) -> Result<(), String> {
    if limits.max_file_bytes == 0
        || limits.max_server_bytes == 0
        || limits.max_file_bytes > i64::MAX as u64
        || limits.max_server_bytes > i64::MAX as u64
    {
        Err("Los límites deben ser positivos y representables en bytes".into())
    } else {
        Ok(())
    }
}
fn internal_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn safe_channel(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn remove_if_exists(path: PathBuf) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("No se pudo eliminar el archivo: {error}")),
    }
}
