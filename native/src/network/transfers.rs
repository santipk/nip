use super::*;
use sha2::{Digest, Sha256};

const CHUNK: usize = 32 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn serve(
    id: u64,
    state: Arc<Mutex<RoomState>>,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) {
    let _ = send.set_priority(-10);
    let result = serve_inner(id, &state, &mut send, &mut recv).await;
    {
        let mut room = state.lock().await;
        if let Some(usage) = room.files.as_ref().and_then(|files| files.usage().ok()) {
            room.broadcast(Response::StorageUsage(usage));
        }
    }
    if let Err(error) = result {
        let _ = write_frame(&mut send, &TransferResponse::Error(error)).await;
    }
    let _ = send.finish();
}

async fn serve_inner(
    id: u64,
    state: &Arc<Mutex<RoomState>>,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
) -> Result<(), String> {
    let request = tokio::time::timeout(IO_TIMEOUT, read_frame::<TransferRequest>(recv))
        .await
        .map_err(|_| "La transferencia no envió sus metadatos")??;
    let (files, gate) = {
        let room = state.lock().await;
        if !room.peers.contains_key(&id) {
            return Err("Participante desconectado".into());
        }
        (
            room.files
                .clone()
                .ok_or("Almacenamiento de adjuntos no disponible")?,
            room.mutations.clone(),
        )
    };
    match request {
        TransferRequest::Upload {
            operation_id,
            channel_id,
            name,
            size,
            sha256,
            text,
        } => {
            if text.chars().count() > 2000 {
                return Err("El texto supera 2000 caracteres".into());
            }
            let persistent = {
                let room = state.lock().await;
                if !room
                    .channels
                    .iter()
                    .any(|c| c.id == channel_id && c.kind == ChannelKind::Text)
                {
                    return Err("El canal de texto no existe".into());
                }
                room.persistent
            };
            let store = files.clone();
            let operation = operation_id.clone();
            let channel = channel_id.clone();
            let upload = tokio::task::spawn_blocking(move || {
                store.begin(operation, channel, name, size, sha256, persistent)
            })
            .await
            .map_err(|_| "Se interrumpió la reserva")??;
            let mut upload = match upload {
                crate::files::BeginUpload::Committed(_) => {
                    let _guard = gate.lock().await;
                    let message = files
                        .publication(&operation_id)?
                        .ok_or("Falta la confirmación del archivo")?;
                    let history = state
                        .lock()
                        .await
                        .history_store
                        .clone()
                        .ok_or("Historial no disponible")?;
                    history.ensure_message(message.clone()).await?;
                    for attachment in &message.attachments {
                        files.mark_published(&attachment.id)?;
                    }
                    {
                        let mut room = state.lock().await;
                        if !room.history.iter().any(|m| m.id == message.id) {
                            room.commit_chat(message.clone());
                        }
                    }
                    write_frame(send, &TransferResponse::Complete(message)).await?;
                    return Ok(());
                }
                crate::files::BeginUpload::Pending(upload) => upload,
            };
            {
                let mut room = state.lock().await;
                room.broadcast(Response::StorageUsage(files.usage()?));
            }
            write_frame(send, &TransferResponse::Ready).await?;
            let mut remaining = size;
            while remaining > 0 {
                let mut buffer = vec![0u8; CHUNK.min(remaining as usize)];
                tokio::time::timeout(IO_TIMEOUT, recv.read_exact(&mut buffer))
                    .await
                    .map_err(|_| "La carga no avanzó durante 30 s")?
                    .map_err(|_| "Carga interrumpida")?;
                remaining -= buffer.len() as u64;
                upload = tokio::task::spawn_blocking(move || {
                    upload.write(&buffer)?;
                    Ok::<_, String>(upload)
                })
                .await
                .map_err(|_| "Se interrumpió la escritura")??;
                tokio::task::yield_now().await;
            }
            let mut extra = [0u8; 1];
            if tokio::time::timeout(IO_TIMEOUT, recv.read(&mut extra))
                .await
                .map_err(|_| "Falta el cierre de la carga")?
                .map_err(|_| "Carga interrumpida")?
                .is_some()
            {
                return Err("El archivo excede la longitud declarada".into());
            }
            let _guard = gate.lock().await;
            let message = {
                let mut room = state.lock().await;
                if !room.persistent {
                    return Err("El servidor dejó de ser persistente".into());
                }
                room.prepare_chat(id, text, channel_id, &mut VecDeque::new())?
            };
            tokio::task::spawn_blocking(move || upload.commit(message))
                .await
                .map_err(|_| "Se interrumpió la confirmación")??;
            let message = files
                .publication(&operation_id)?
                .ok_or("No se confirmó el mensaje del archivo")?;
            let history = state
                .lock()
                .await
                .history_store
                .clone()
                .ok_or("Historial no disponible")?;
            history.ensure_message(message.clone()).await?;
            for attachment in &message.attachments {
                files.mark_published(&attachment.id)?;
            }
            {
                let mut room = state.lock().await;
                room.commit_chat(message.clone());
                room.broadcast(Response::StorageUsage(files.usage()?));
            }
            write_frame(send, &TransferResponse::Complete(message)).await?;
        }
        TransferRequest::Download {
            channel_id,
            attachment_id,
        } => {
            {
                let room = state.lock().await;
                if !room
                    .channels
                    .iter()
                    .any(|c| c.id == channel_id && c.kind == ChannelKind::Text)
                {
                    return Err("El canal de texto no existe".into());
                }
            }
            let (metadata, file) = tokio::task::spawn_blocking(move || {
                files.open_attachment(&channel_id, &attachment_id)
            })
            .await
            .map_err(|_| "Se interrumpió la descarga")??;
            let mut file = tokio::fs::File::from_std(file);
            write_frame(send, &TransferResponse::File(metadata.clone())).await?;
            let mut buffer = vec![0u8; CHUNK];
            let mut remaining = metadata.size;
            while remaining > 0 {
                let count = file
                    .read(&mut buffer[..CHUNK.min(remaining as usize)])
                    .await
                    .map_err(|e| e.to_string())?;
                if count == 0 {
                    return Err("El archivo alojado está incompleto".into());
                }
                tokio::time::timeout(IO_TIMEOUT, send.write_all(&buffer[..count]))
                    .await
                    .map_err(|_| "La descarga no avanzó durante 30 s")?
                    .map_err(|_| "Descarga interrumpida")?;
                remaining -= count as u64;
                tokio::task::yield_now().await;
            }
        }
    }
    Ok(())
}

fn progress(
    events: &std::sync::mpsc::Sender<Event>,
    operation_id: &str,
    name: &str,
    completed: u64,
    total: u64,
    finished: bool,
    error: Option<String>,
) {
    let _ = events.send(Event::Transfer(TransferStatus {
        operation_id: operation_id.into(),
        name: name.into(),
        completed,
        total,
        finished,
        error,
    }));
}

pub(super) async fn upload(
    connection: quinn::Connection,
    path: PathBuf,
    channel_id: String,
    operation_id: String,
    text: String,
    events: std::sync::mpsc::Sender<Event>,
    finished: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Nombre de archivo inválido")?
        .to_owned();
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("No se pudo abrir el archivo: {e}"))?;
    let metadata = file.metadata().await.map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Elegí un archivo regular".into());
    }
    let size = metadata.len();
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    progress(&events, &operation_id, &name, 0, size, false, None);
    loop {
        let count = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    file.seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|e| e.to_string())?;
    let (mut send, mut recv) = connection.open_bi().await.map_err(|_| "Conexión cerrada")?;
    let _ = send.set_priority(-10);
    write_frame(
        &mut send,
        &TransferRequest::Upload {
            operation_id: operation_id.clone(),
            channel_id,
            name: name.clone(),
            size,
            sha256: format!("{:x}", hash.finalize()),
            text,
        },
    )
    .await?;
    let response = tokio::time::timeout(IO_TIMEOUT, read_frame::<TransferResponse>(&mut recv))
        .await
        .map_err(|_| "El anfitrión no respondió a la carga")??;
    match response {
        TransferResponse::Complete(_) => {
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
            progress(&events, &operation_id, &name, size, size, true, None);
            return Ok(());
        }
        TransferResponse::Ready => {}
        TransferResponse::Error(error) => return Err(error),
        _ => return Err("Respuesta de carga inválida".into()),
    }
    let mut sent = 0u64;
    while sent < size {
        let count = file
            .read(&mut buffer[..CHUNK.min((size - sent) as usize)])
            .await
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("El archivo cambió durante la carga".into());
        }
        tokio::time::timeout(IO_TIMEOUT, send.write_all(&buffer[..count]))
            .await
            .map_err(|_| "La carga no avanzó durante 30 s")?
            .map_err(|_| "Carga interrumpida")?;
        sent += count as u64;
        progress(&events, &operation_id, &name, sent, size, false, None);
        tokio::task::yield_now().await;
    }
    send.finish().map_err(|_| "Carga interrumpida")?;
    match tokio::time::timeout(IO_TIMEOUT, read_frame::<TransferResponse>(&mut recv))
        .await
        .map_err(|_| "Se perdió la confirmación; reintentá el mismo envío")??
    {
        TransferResponse::Complete(_) => {
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
            progress(&events, &operation_id, &name, size, size, true, None);
            Ok(())
        }
        TransferResponse::Error(error) => Err(error),
        _ => Err("Confirmación de archivo inválida".into()),
    }
}

pub(super) async fn download(
    connection: quinn::Connection,
    channel_id: String,
    attachment_id: String,
    destination: PathBuf,
    operation_id: String,
    events: std::sync::mpsc::Sender<Event>,
    finished: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    if destination.exists() {
        return Err("El archivo de destino ya existe; elegí otro nombre".into());
    }
    let (mut send, mut recv) = connection.open_bi().await.map_err(|_| "Conexión cerrada")?;
    let _ = send.set_priority(-10);
    write_frame(
        &mut send,
        &TransferRequest::Download {
            channel_id: channel_id.clone(),
            attachment_id: attachment_id.clone(),
        },
    )
    .await?;
    send.finish().map_err(|_| "Conexión cerrada")?;
    let metadata = match tokio::time::timeout(IO_TIMEOUT, read_frame::<TransferResponse>(&mut recv))
        .await
        .map_err(|_| "El anfitrión no respondió a la descarga")??
    {
        TransferResponse::File(metadata) => metadata,
        TransferResponse::Error(error) => return Err(error),
        _ => return Err("Respuesta de descarga inválida".into()),
    };
    if metadata.id != attachment_id || metadata.channel_id != channel_id {
        return Err("El adjunto no coincide con el canal solicitado".into());
    }
    let pending = destination.with_file_name(format!(
        ".nip-download-{:016x}",
        getrandom::u64().map_err(|e| e.to_string())?
    ));
    let cleanup = PendingFile(pending.clone());
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .await
        .map_err(|e| e.to_string())?;
    let mut received = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    progress(
        &events,
        &operation_id,
        &metadata.name,
        0,
        metadata.size,
        false,
        None,
    );
    while received < metadata.size {
        let count = CHUNK.min((metadata.size - received) as usize);
        tokio::time::timeout(IO_TIMEOUT, recv.read_exact(&mut buffer[..count]))
            .await
            .map_err(|_| "La descarga no avanzó durante 30 s")?
            .map_err(|_| "Descarga incompleta")?;
        file.write_all(&buffer[..count])
            .await
            .map_err(|e| e.to_string())?;
        hash.update(&buffer[..count]);
        received += count as u64;
        progress(
            &events,
            &operation_id,
            &metadata.name,
            received,
            metadata.size,
            false,
            None,
        );
    }
    let mut extra = [0u8; 1];
    if tokio::time::timeout(IO_TIMEOUT, recv.read(&mut extra))
        .await
        .map_err(|_| "Falta el cierre de la descarga")?
        .map_err(|_| "Descarga interrumpida")?
        .is_some()
        || format!("{:x}", hash.finalize()) != metadata.sha256
    {
        return Err("Longitud o hash del archivo incorrectos".into());
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    drop(file);
    if destination.exists() {
        return Err("El destino ya existe".into());
    }
    tokio::fs::rename(&pending, &destination)
        .await
        .map_err(|e| e.to_string())?;
    drop(cleanup);
    finished.store(true, std::sync::atomic::Ordering::SeqCst);
    progress(
        &events,
        &operation_id,
        &metadata.name,
        received,
        metadata.size,
        true,
        None,
    );
    Ok(())
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

use tokio::io::AsyncSeekExt;
