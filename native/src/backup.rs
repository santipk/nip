//! Password-encrypted portable backups. Plaintext records never use the filesystem.
use crate::{
    contracts::{Attachment, Channel, ChannelKind, ChatMessage},
    files::{AttachmentStore, Limits},
    history::History,
    storage::{Store, protect, unprotect},
};
use age::secrecy::{ExposeSecret, SecretString};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub servers: u64,
    pub messages: u64,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub file_bytes: u64,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    servers: Vec<ServerManifest>,
    inventory: Inventory,
}
#[derive(Serialize, Deserialize)]
struct ServerManifest {
    id: String,
    channels: Vec<Channel>,
    counters: Vec<(String, i64)>,
    messages: u64,
    #[serde(default)]
    files: u64,
    #[serde(default)]
    file_bytes: u64,
}
#[derive(Serialize, Deserialize)]
struct AttachmentRecord {
    metadata: Attachment,
    operation_id: String,
    publication: ChatMessage,
    published: bool,
}

struct TemporaryDirectory(PathBuf);
impl TemporaryDirectory {
    fn create(parent: &Path) -> Result<Self, String> {
        let path = parent.join(format!(
            ".nip-backup-{:016x}",
            getrandom::u64().map_err(|e| e.to_string())?
        ));
        std::fs::create_dir(&path)
            .map_err(|e| format!("No se pudo crear la carpeta temporal: {e}"))?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Export must be invoked with the host's data mutation gate held. Each SQLite snapshot includes WAL.
pub async fn export(
    store: &Store,
    destination: PathBuf,
    passphrase: SecretString,
) -> Result<Inventory, String> {
    export_gated(store, destination, passphrase, Vec::new()).await
}

pub async fn export_gated(
    store: &Store,
    destination: PathBuf,
    passphrase: SecretString,
    gates: Vec<std::sync::Arc<tokio::sync::Mutex<()>>>,
) -> Result<Inventory, String> {
    if passphrase.expose_secret().is_empty() {
        return Err("Elegí una contraseña para el respaldo".into());
    }
    if destination.exists() {
        return Err("El archivo de destino ya existe".into());
    }
    let mut guards = Vec::new();
    for gate in gates {
        guards.push(gate.lock_owned().await);
    }
    let catalog = store.portable_catalog()?;
    let temporary = TemporaryDirectory::create(&std::env::temp_dir())?;
    let mut snapshots = Vec::new();
    for server in store.local_servers() {
        let id = server.identity.id();
        let source = store.history_path(&id)?;
        let target = temporary.0.join(format!("{id}.sqlite3"));
        if source.exists() {
            History::open(source.clone())
                .await?
                .snapshot(target.clone())
                .await?;
        } else {
            // A session server may not have a database yet; persist only its initial channel catalog.
            drop(History::open(target.clone()).await?);
        }
        let server_directory = source.parent().ok_or("Servidor sin carpeta")?.to_owned();
        let files = tokio::task::spawn_blocking(move || snapshot_attachments(server_directory))
            .await
            .map_err(|_| "Se interrumpió snapshot de adjuntos")??;
        snapshots.push((id, target, files));
    }
    drop(guards);
    tokio::task::spawn_blocking(move || {
        let _temporary = temporary;
        let mut manifest = Manifest {
            version: 2,
            servers: Vec::new(),
            inventory: Inventory::default(),
        };
        for (id, path, files_snapshot) in &snapshots {
            let database = Connection::open(path).map_err(|e| e.to_string())?;
            let mut channels = database
                .prepare("SELECT id,name,kind FROM channels ORDER BY rowid")
                .map_err(|e| e.to_string())?;
            let channels = channels
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
                .map_err(|e| e.to_string())?;
            let mut counters = database
                .prepare("SELECT key,value FROM counters")
                .map_err(|e| e.to_string())?;
            let counters = counters
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let messages = database
                .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| e.to_string())? as u64;
            manifest.inventory.messages += messages;
            let (files,file_bytes)=if let Some(snapshot)=files_snapshot {
                Connection::open(snapshot.0.join("index.sqlite3")).map_err(|e|e.to_string())?.query_row("SELECT COUNT(*),COALESCE(SUM(size),0) FROM attachments WHERE state!='uploading'",[],|row|Ok((row.get::<_,i64>(0)? as u64,row.get::<_,i64>(1)? as u64))).map_err(|e|e.to_string())?
            } else {(0,0)};
            manifest.inventory.files+=files; manifest.inventory.file_bytes+=file_bytes;
            manifest.servers.push(ServerManifest {
                id: id.clone(),
                channels,
                counters,
                messages,
                files,
                file_bytes,
            });
        }
        manifest.inventory.servers = manifest.servers.len() as u64;
        validate_manifest(&manifest)?;
        let parent = destination.parent().ok_or("Destino sin carpeta")?;
        let output_temporary = TemporaryDirectory::create(parent)?;
        let pending = output_temporary.0.join("backup.age");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|e| e.to_string())?;
        let writer = age::Encryptor::with_user_passphrase(passphrase)
            .wrap_output(file)
            .map_err(|e| e.to_string())?;
        let mut archive = tar::Builder::new(writer);
        archive.follow_symlinks(false);
        append_json(&mut archive, "manifest.json", &manifest)?;
        append_bytes(&mut archive, "catalog.json", &catalog)?;
        for (id, path, files_snapshot) in snapshots {
            let database = Connection::open(path).map_err(|e| e.to_string())?;
            let mut query = database
                .prepare("SELECT id,channel_id,payload FROM messages ORDER BY id")
                .map_err(|e| e.to_string())?;
            let rows = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (message_id, channel_id, payload) = row.map_err(|e| e.to_string())?;
                let mut message: ChatMessage = serde_json::from_slice(&unprotect(&payload)?)
                    .map_err(|_| "Historial dañado durante el respaldo")?;
                message.channel_id = channel_id;
                append_json(
                    &mut archive,
                    &format!("messages/{id}/{message_id}.json"),
                    &message,
                )?;
            }
            if let Some(snapshot)=files_snapshot { write_attachments(&mut archive,&id,&snapshot)?; }
        }
        let writer = archive.into_inner().map_err(|e| e.to_string())?;
        let file = writer.finish().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        if destination.exists() {
            return Err("El archivo de destino ya existe".into());
        }
        std::fs::rename(pending, destination)
            .map_err(|e| format!("No se pudo publicar el respaldo: {e}"))?;
        Ok(manifest.inventory)
    })
    .await
    .map_err(|_| "Se interrumpió el respaldo".to_string())?
}

pub async fn restore(
    archive: PathBuf,
    destination: PathBuf,
    passphrase: SecretString,
) -> Result<Inventory, String> {
    tokio::task::spawn_blocking(move || restore_blocking(archive, destination, passphrase))
        .await
        .map_err(|_| "Se interrumpió la restauración".to_string())?
}

fn restore_blocking(
    archive: PathBuf,
    destination: PathBuf,
    passphrase: SecretString,
) -> Result<Inventory, String> {
    if destination.exists() {
        return Err("La restauración requiere un perfil nuevo; el destino ya existe".into());
    }
    let parent = destination.parent().ok_or("Destino sin carpeta")?;
    let staging = TemporaryDirectory::create(parent)?;
    let decryptor = age::Decryptor::new(File::open(archive).map_err(|e| e.to_string())?)
        .map_err(|_| "Respaldo cifrado inválido")?;
    if !decryptor.is_scrypt() {
        return Err("Este respaldo no usa contraseña".into());
    }
    let identity = age::scrypt::Identity::new(passphrase);
    let reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|_| "Contraseña incorrecta o respaldo dañado")?;
    let mut archive = tar::Archive::new(reader);
    let mut entries = archive.entries().map_err(|e| e.to_string())?;
    let manifest_entry = entries
        .next()
        .ok_or("Falta el manifest del respaldo")?
        .map_err(|e| e.to_string())?;
    let manifest: Manifest = read_json(manifest_entry, "manifest.json", 2 * 1024 * 1024)?;
    validate_manifest(&manifest)?;
    let catalog_entry = entries
        .next()
        .ok_or("Falta el catálogo del respaldo")?
        .map_err(|e| e.to_string())?;
    let catalog = read_entry(catalog_entry, "catalog.json", 16 * 1024 * 1024)?;
    let store = Store::restore_catalog(staging.0.clone(), &catalog)?;
    let hosts: HashSet<_> = store
        .local_servers()
        .iter()
        .map(|s| s.identity.id())
        .collect();
    if hosts != manifest.servers.iter().map(|s| s.id.clone()).collect() {
        return Err("El inventario no coincide con el catálogo".into());
    }
    let mut databases = HashMap::new();
    let mut attachment_databases = HashMap::new();
    let mut counts = HashMap::<String, u64>::new();
    let mut file_counts = HashMap::<String, (u64, u64)>::new();
    for server in &manifest.servers {
        let path = store.history_path(&server.id)?;
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let database = Connection::open(path).map_err(|e| e.to_string())?;
        database
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        database.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE messages(id INTEGER PRIMARY KEY,sender_id INTEGER NOT NULL,payload BLOB NOT NULL,channel_id TEXT NOT NULL DEFAULT 'general'); CREATE INDEX messages_channel_id ON messages(channel_id,id); CREATE TABLE channels(id TEXT PRIMARY KEY,name TEXT NOT NULL,kind TEXT NOT NULL CHECK(kind IN ('text','voice'))); CREATE TABLE counters(key TEXT PRIMARY KEY,value INTEGER NOT NULL); BEGIN IMMEDIATE;").map_err(|e| e.to_string())?;
        for channel in &server.channels {
            database
                .execute(
                    "INSERT INTO channels VALUES (?1,?2,?3)",
                    params![
                        channel.id,
                        channel.name,
                        if channel.kind == ChannelKind::Text {
                            "text"
                        } else {
                            "voice"
                        }
                    ],
                )
                .map_err(|e| e.to_string())?;
        }
        for (key, value) in &server.counters {
            database
                .execute("INSERT INTO counters VALUES (?1,?2)", params![key, value])
                .map_err(|e| e.to_string())?;
        }
        databases.insert(server.id.clone(), database);
        counts.insert(server.id.clone(), 0);
        if server.files > 0 {
            let server_directory = store.history_path(&server.id)?.parent().unwrap().to_owned();
            let settings = store.local_server(&server.id).unwrap().settings;
            drop(AttachmentStore::open(
                server_directory.clone(),
                Limits {
                    max_file_bytes: settings.file_limit_bytes,
                    max_server_bytes: settings.storage_limit_bytes,
                },
            )?);
            attachment_databases.insert(
                server.id.clone(),
                Connection::open(server_directory.join("attachments/index.sqlite3"))
                    .map_err(|e| e.to_string())?,
            );
        }
        file_counts.insert(server.id.clone(), (0, 0));
    }
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_str()
            .ok_or("Ruta de respaldo inválida")?
            .to_owned();
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 3 || !["messages", "attachments"].contains(&parts[0]) {
            return Err("Entrada de respaldo desconocida".into());
        }
        let server = manifest
            .servers
            .iter()
            .find(|s| s.id == parts[1])
            .ok_or("Servidor de mensaje desconocido")?;
        if parts[0] == "attachments" {
            let record: AttachmentRecord = read_json(entry, &path, 65536)?;
            validate_attachment_record(&record, server)?;
            if parts[2] != format!("{}.json", record.metadata.id) {
                return Err("Registro de adjunto inválido".into());
            }
            let count = file_counts.get_mut(&server.id).unwrap();
            count.0 += 1;
            count.1 = count
                .1
                .checked_add(record.metadata.size)
                .ok_or("Inventario de archivos demasiado grande")?;
            if count.0 > server.files || count.1 > server.file_bytes {
                return Err("El respaldo supera el inventario de adjuntos".into());
            }
            let blob = entries
                .next()
                .ok_or("Falta el contenido de un adjunto")?
                .map_err(|e| e.to_string())?;
            let directory = store
                .history_path(&server.id)?
                .parent()
                .unwrap()
                .join("attachments/blobs");
            restore_blob(
                blob,
                &format!("blobs/{}/{}", server.id, record.metadata.id),
                &record.metadata,
                &directory,
            )?;
            let publication =
                protect(&serde_json::to_vec(&record.publication).map_err(|e| e.to_string())?)?;
            attachment_databases.get(&server.id).ok_or("Adjunto no incluido en inventario")?.execute("INSERT INTO attachments(id,operation_id,channel_id,name,size,sha256,state,publication) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![record.metadata.id,record.operation_id,record.metadata.channel_id,record.metadata.name,record.metadata.size as i64,record.metadata.sha256,if record.published {"published"}else{"ready"},publication]).map_err(|e|e.to_string())?;
            continue;
        }
        let message: ChatMessage = read_json(entry, &path, 65536)?;
        if parts[2] != format!("{}.json", message.id)
            || message.id == 0
            || message.id >= i64::MAX as u64
            || message.sender_id == 0
            || message.sender_id >= i64::MAX as u64
            || message.sender_name.trim().is_empty()
            || message.sender_name.chars().count() > 64
            || (message.text.trim().is_empty() && message.attachments.is_empty())
            || message.text.chars().count() > 2000
            || !server
                .channels
                .iter()
                .any(|c| c.id == message.channel_id && c.kind == ChannelKind::Text)
        {
            return Err("Mensaje de respaldo inválido".into());
        }
        let count = counts.get_mut(&server.id).unwrap();
        *count += 1;
        if *count > server.messages {
            return Err("El respaldo contiene más mensajes que el inventario".into());
        }
        let payload = protect(&serde_json::to_vec(&message).map_err(|e| e.to_string())?)?;
        databases[&server.id]
            .execute(
                "INSERT INTO messages VALUES (?1,?2,?3,?4)",
                params![
                    message.id as i64,
                    message.sender_id as i64,
                    payload,
                    message.channel_id
                ],
            )
            .map_err(|e| format!("Mensaje duplicado o inválido: {e}"))?;
    }
    // tar stops at its terminator. Read the remaining age stream to verify its final authentication tag.
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink())
        .map_err(|_| "Respaldo truncado o manipulado")?;
    for server in &manifest.servers {
        if counts[&server.id] != server.messages {
            return Err("El inventario de mensajes está incompleto".into());
        }
        if file_counts[&server.id] != (server.files, server.file_bytes) {
            return Err("El inventario de adjuntos está incompleto".into());
        }
        validate_restored_references(&databases[&server.id], attachment_databases.get(&server.id))?;
    }
    for database in databases.values() {
        let integrity: String = database
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if integrity != "ok" {
            return Err("La base restaurada no superó integridad".into());
        }
        database
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;
    }
    drop(databases);
    drop(attachment_databases);
    drop(store);
    if destination.exists() {
        return Err("El destino de restauración ya existe".into());
    }
    std::fs::rename(&staging.0, destination)
        .map_err(|e| format!("No se pudo completar la restauración: {e}"))?;
    Ok(manifest.inventory)
}

fn validate_manifest(manifest: &Manifest) -> Result<(), String> {
    if ![1, 2].contains(&manifest.version)
        || manifest.servers.len() > 1024
        || manifest.inventory.servers != manifest.servers.len() as u64
    {
        return Err("Versión o inventario de respaldo incompatible".into());
    }
    let mut ids = HashSet::new();
    let mut messages = 0u64;
    let mut files = 0u64;
    let mut file_bytes = 0u64;
    for server in &manifest.servers {
        if server.id.len() != 43
            || !safe_id(&server.id)
            || !ids.insert(&server.id)
            || server.channels.is_empty()
            || server.channels.len() > 128
        {
            return Err("Servidor de respaldo inválido".into());
        }
        let mut channels = HashSet::new();
        for channel in &server.channels {
            if channel.id.len() > 64
                || !safe_id(&channel.id)
                || !channels.insert(&channel.id)
                || channel.name.trim().is_empty()
                || channel.name.chars().count() > 64
                || channel.name.chars().any(char::is_control)
            {
                return Err("Canal de respaldo inválido".into());
            }
        }
        if !server.channels.iter().any(|c| c.kind == ChannelKind::Text) {
            return Err("El respaldo no tiene canal de texto".into());
        }
        let mut keys = HashSet::new();
        for (key, value) in &server.counters {
            if !["message", "peer"].contains(&key.as_str())
                || *value < 0
                || *value == i64::MAX
                || !keys.insert(key)
            {
                return Err("Contadores de respaldo inválidos".into());
            }
        }
        messages = messages
            .checked_add(server.messages)
            .ok_or("Inventario demasiado grande")?;
        files = files
            .checked_add(server.files)
            .ok_or("Inventario demasiado grande")?;
        file_bytes = file_bytes
            .checked_add(server.file_bytes)
            .ok_or("Inventario demasiado grande")?;
    }
    if messages != manifest.inventory.messages
        || files != manifest.inventory.files
        || file_bytes != manifest.inventory.file_bytes
    {
        return Err("Inventario de mensajes inválido".into());
    }
    Ok(())
}
fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_restored_references(
    history: &Connection,
    files: Option<&Connection>,
) -> Result<(), String> {
    let mut query = history
        .prepare("SELECT payload FROM messages")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let message: ChatMessage =
            serde_json::from_slice(&unprotect(&row.map_err(|e| e.to_string())?)?)
                .map_err(|_| "Mensaje restaurado inválido")?;
        for attachment in &message.attachments {
            let files = files.ok_or("El historial referencia adjuntos ausentes")?;
            let payload: Vec<u8> = files
                .query_row(
                    "SELECT publication FROM attachments WHERE id=?1 AND channel_id=?2",
                    params![attachment.id, message.channel_id],
                    |row| row.get(0),
                )
                .map_err(|_| "El historial referencia un adjunto ausente")?;
            let publication: ChatMessage = serde_json::from_slice(&unprotect(&payload)?)
                .map_err(|_| "Publicación restaurada inválida")?;
            if serde_json::to_value(&publication).map_err(|e| e.to_string())?
                != serde_json::to_value(&message).map_err(|e| e.to_string())?
            {
                return Err("La publicación del adjunto no coincide con su mensaje".into());
            }
        }
    }
    if let Some(files) = files {
        let mut query = files
            .prepare("SELECT publication,state FROM attachments")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (payload, state) = row.map_err(|e| e.to_string())?;
            let publication: ChatMessage = serde_json::from_slice(&unprotect(&payload)?)
                .map_err(|_| "Publicación restaurada inválida")?;
            let stored: Option<Vec<u8>> = history
                .query_row(
                    "SELECT payload FROM messages WHERE id=?1",
                    [publication.id as i64],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(stored) = stored {
                let message: ChatMessage = serde_json::from_slice(&unprotect(&stored)?)
                    .map_err(|_| "Mensaje restaurado inválido")?;
                if serde_json::to_value(message).map_err(|e| e.to_string())?
                    != serde_json::to_value(publication).map_err(|e| e.to_string())?
                {
                    return Err("Una publicación pendiente colisiona con otro mensaje".into());
                }
            } else if state == "published" {
                return Err("Un adjunto publicado no tiene mensaje".into());
            }
        }
    }
    Ok(())
}

fn snapshot_attachments(server_directory: PathBuf) -> Result<Option<TemporaryDirectory>, String> {
    let source = server_directory.join("attachments/index.sqlite3");
    if !source.exists() {
        return Ok(None);
    }
    let temporary = TemporaryDirectory::create(&server_directory)?;
    std::fs::create_dir(temporary.0.join("blobs")).map_err(|e| e.to_string())?;
    let database = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let mut target =
        Connection::open(temporary.0.join("index.sqlite3")).map_err(|e| e.to_string())?;
    rusqlite::backup::Backup::new(&database, &mut target)
        .map_err(|e| e.to_string())?
        .run_to_completion(128, Duration::from_millis(5), None)
        .map_err(|e| e.to_string())?;
    let mut query = target
        .prepare("SELECT id FROM attachments WHERE state!='uploading'")
        .map_err(|e| e.to_string())?;
    let ids = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for id in ids {
        let id = id.map_err(|e| e.to_string())?;
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Índice de adjuntos dañado".into());
        }
        let source = server_directory.join("attachments/blobs").join(&id);
        let destination = temporary.0.join("blobs").join(&id);
        // The link keeps a captured blob available if its channel is deleted while age encrypts.
        if std::fs::hard_link(&source, &destination).is_err() {
            std::fs::copy(source, destination)
                .map_err(|e| format!("No se pudo respaldar el blob: {e}"))?;
        }
    }
    Ok(Some(temporary))
}

fn write_attachments<W: Write>(
    archive: &mut tar::Builder<W>,
    server_id: &str,
    snapshot: &TemporaryDirectory,
) -> Result<(), String> {
    let database = Connection::open(snapshot.0.join("index.sqlite3")).map_err(|e| e.to_string())?;
    let mut query=database.prepare("SELECT id,channel_id,name,size,sha256,operation_id,publication,state FROM attachments WHERE state!='uploading' ORDER BY rowid").map_err(|e|e.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok((
                Attachment {
                    id: row.get(0)?,
                    channel_id: row.get(1)?,
                    name: row.get(2)?,
                    size: row.get::<_, i64>(3)? as u64,
                    sha256: row.get(4)?,
                },
                row.get::<_, String>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (metadata, operation_id, publication, state) = row.map_err(|e| e.to_string())?;
        let publication = serde_json::from_slice(&unprotect(&publication)?)
            .map_err(|_| "Publicación de adjunto dañada")?;
        let record = AttachmentRecord {
            metadata,
            operation_id,
            publication,
            published: state == "published",
        };
        append_json(
            archive,
            &format!("attachments/{server_id}/{}.json", record.metadata.id),
            &record,
        )?;
        let file = File::open(snapshot.0.join("blobs").join(&record.metadata.id))
            .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() != record.metadata.size {
            return Err("Longitud del adjunto inconsistente".into());
        }
        let mut reader = HashingReader {
            reader: file,
            hash: Sha256::new(),
            bytes: 0,
        };
        let mut header = tar::Header::new_gnu();
        header.set_size(record.metadata.size);
        header.set_mode(0o600);
        header.set_cksum();
        archive
            .append_data(
                &mut header,
                format!("blobs/{server_id}/{}", record.metadata.id),
                &mut reader,
            )
            .map_err(|e| e.to_string())?;
        if reader.bytes != record.metadata.size
            || format!("{:x}", reader.hash.finalize()) != record.metadata.sha256
        {
            return Err("Hash del adjunto inconsistente; respaldo no publicado".into());
        }
    }
    Ok(())
}

fn validate_attachment_record(
    record: &AttachmentRecord,
    server: &ServerManifest,
) -> Result<(), String> {
    let metadata = &record.metadata;
    if metadata.id.len() != 32
        || !metadata.id.bytes().all(|b| b.is_ascii_hexdigit())
        || !(32..=64).contains(&record.operation_id.len())
        || !record.operation_id.bytes().all(|b| b.is_ascii_hexdigit())
        || metadata.size > i64::MAX as u64
        || metadata.sha256.len() != 64
        || !metadata.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || metadata.name.is_empty()
        || metadata.name.chars().count() > 255
        || metadata
            .name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !server
            .channels
            .iter()
            .any(|c| c.id == metadata.channel_id && c.kind == ChannelKind::Text)
        || record.publication.channel_id != metadata.channel_id
        || record.publication.id == 0
        || record.publication.id >= i64::MAX as u64
        || record.publication.sender_id == 0
        || record.publication.sender_id >= i64::MAX as u64
        || record.publication.sender_name.trim().is_empty()
        || record.publication.sender_name.chars().count() > 64
        || record.publication.text.chars().count() > 2000
        || record.publication.attachments.len() != 1
        || record.publication.attachments[0] != *metadata
    {
        return Err("Metadatos o publicación de adjunto inválidos".into());
    }
    Ok(())
}
fn restore_blob<R: Read>(
    entry: tar::Entry<'_, R>,
    path: &str,
    metadata: &Attachment,
    directory: &Path,
) -> Result<(), String> {
    if !entry.header().entry_type().is_file()
        || entry.path().map_err(|e| e.to_string())? != Path::new(path)
        || entry.size() != metadata.size
    {
        return Err("Contenido de adjunto inválido".into());
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join(&metadata.id))
        .map_err(|e| e.to_string())?;
    let mut reader = HashingReader {
        reader: entry,
        hash: Sha256::new(),
        bytes: 0,
    };
    std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
    if reader.bytes != metadata.size || format!("{:x}", reader.hash.finalize()) != metadata.sha256 {
        return Err("Longitud o hash de adjunto inválido".into());
    }
    file.sync_all().map_err(|e| e.to_string())
}
struct HashingReader<R> {
    reader: R,
    hash: Sha256,
    bytes: u64,
}
impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(buffer)?;
        self.hash.update(&buffer[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
}
fn append_json<W: Write, T: Serialize>(
    archive: &mut tar::Builder<W>,
    path: &str,
    value: &T,
) -> Result<(), String> {
    append_bytes(
        archive,
        path,
        &serde_json::to_vec(value).map_err(|e| e.to_string())?,
    )
}
fn append_bytes<W: Write>(
    archive: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .map_err(|e| e.to_string())
}
fn read_json<R: Read, T: serde::de::DeserializeOwned>(
    entry: tar::Entry<'_, R>,
    path: &str,
    limit: u64,
) -> Result<T, String> {
    serde_json::from_slice(&read_entry(entry, path, limit)?)
        .map_err(|_| "Registro de respaldo inválido".into())
}
fn read_entry<R: Read>(
    mut entry: tar::Entry<'_, R>,
    path: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    if !entry.header().entry_type().is_file()
        || entry.path().map_err(|e| e.to_string())? != Path::new(path)
        || entry.size() > limit
    {
        return Err("Entrada de respaldo inválida o demasiado grande".into());
    }
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|_| "Respaldo truncado o manipulado")?;
    Ok(bytes)
}
