#![cfg(windows)]
use age::secrecy::SecretString;
use nip_native::{backup, contracts::*, history::History, storage::Store};
use std::path::PathBuf;

fn directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nip-backup-{label}-{:016x}",
        getrandom::u64().unwrap()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

#[tokio::test]
async fn wrong_password_corruption_and_truncation_never_publish_a_profile() {
    let root = directory("failures");
    let mut store = Store::open(root.join("original")).unwrap();
    store
        .create_server("QA", LocalServerSettings::default())
        .unwrap();
    let archive = root.join("valid.age");
    backup::export(
        &store,
        archive.clone(),
        SecretString::from("contraseña QA".to_owned()),
    )
    .await
    .unwrap();
    let target = root.join("wrong");
    assert!(
        backup::restore(
            archive.clone(),
            target.clone(),
            SecretString::from("incorrecta".to_owned())
        )
        .await
        .is_err()
    );
    assert!(!target.exists());
    let bytes = std::fs::read(&archive).unwrap();
    let truncated = root.join("truncated.age");
    std::fs::write(&truncated, &bytes[..bytes.len() - 20]).unwrap();
    let target = root.join("truncated");
    assert!(
        backup::restore(
            truncated,
            target.clone(),
            SecretString::from("contraseña QA".to_owned())
        )
        .await
        .is_err()
    );
    assert!(!target.exists());
    let corrupt = root.join("corrupt.age");
    let mut bytes = bytes;
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&corrupt, bytes).unwrap();
    let target = root.join("corrupt");
    assert!(
        backup::restore(
            corrupt,
            target.clone(),
            SecretString::from("contraseña QA".to_owned())
        )
        .await
        .is_err()
    );
    assert!(!target.exists());
    assert_eq!(
        std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".nip-backup-"))
            .count(),
        0
    );
    let existing = root.join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("keep"), b"original").unwrap();
    assert!(
        backup::restore(
            archive.clone(),
            existing.clone(),
            SecretString::from("contraseña QA".to_owned())
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(existing.join("keep")).unwrap(), b"original");
    assert!(
        backup::export(
            &store,
            archive,
            SecretString::from("contraseña QA".to_owned())
        )
        .await
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn backup_restores_original_blobs_and_unpublished_outbox() {
    use nip_native::files::{AttachmentStore, BeginUpload, Limits};
    use sha2::{Digest, Sha256};
    let root = directory("blobs");
    let mut store = Store::open(root.join("original")).unwrap();
    let server = store
        .create_server(
            "Archivos",
            LocalServerSettings {
                persistent: true,
                ..Default::default()
            },
        )
        .unwrap();
    let history_path = store.history_path(&server.identity.id()).unwrap();
    let history = History::open(history_path.clone()).await.unwrap();
    let files = AttachmentStore::open(history_path.parent().unwrap().to_owned(), Limits::default())
        .unwrap();
    let mut expected = Vec::new();
    for (index, published) in [(1, true), (2, false)] {
        let operation = format!("{index:032x}");
        let BeginUpload::Pending(mut upload) = files
            .begin(
                operation.clone(),
                "general".into(),
                format!("archivo{index}.txt"),
                4,
                format!("{:x}", Sha256::digest(b"hola")),
                true,
            )
            .unwrap()
        else {
            panic!()
        };
        upload.write(b"hola").unwrap();
        let attachment = upload
            .commit(ChatMessage {
                attachments: vec![],
                channel_id: "general".into(),
                id: index,
                sender_id: 1,
                sender_name: "QA".into(),
                text: String::new(),
                sent_at: 1,
            })
            .unwrap();
        if published {
            history
                .append(files.publication(&operation).unwrap().unwrap())
                .await
                .unwrap();
            files.mark_published(&attachment.id).unwrap();
        }
        expected.push(attachment);
    }
    let archive = root.join("blobs.age");
    let inventory = backup::export(
        &store,
        archive.clone(),
        SecretString::from("contraseña con adjuntos".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(inventory.files, 2);
    assert_eq!(inventory.file_bytes, 8);
    let restored = root.join("restored");
    assert_eq!(
        backup::restore(
            archive,
            restored.clone(),
            SecretString::from("contraseña con adjuntos".to_owned())
        )
        .await
        .unwrap(),
        inventory
    );
    let restored_store = Store::open(restored).unwrap();
    let restored_path = restored_store.history_path(&server.identity.id()).unwrap();
    let restored_files = AttachmentStore::open(
        restored_path.parent().unwrap().to_owned(),
        Limits::default(),
    )
    .unwrap();
    let (_, mut file) = restored_files
        .open_attachment("general", &expected[0].id)
        .unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, b"hola");
    drop(file);
    let pending = restored_files.pending_publications().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].attachments[0].id, expected[1].id);
    let restored_history = History::open(restored_path).await.unwrap();
    assert_eq!(
        restored_history.page(u64::MAX).await.unwrap().0[0].attachments[0].id,
        expected[0].id
    );
    drop(restored_history);
    drop(restored_files);
    drop(history);
    drop(files);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn encrypted_backup_restores_identity_settings_channels_and_live_wal_offline() {
    let root = directory("restore");
    let mut store = Store::open(root.join("original")).unwrap();
    store.save_display_name("QA backup".into()).unwrap();
    let server = store
        .create_server(
            "Comunidad QA",
            LocalServerSettings {
                persistent: true,
                auto_start: true,
                ..Default::default()
            },
        )
        .unwrap();
    let id = server.identity.id();
    let invitation = store.invitation(&id).unwrap().encode().unwrap();
    let history = History::open(store.history_path(&id).unwrap())
        .await
        .unwrap();
    let channel = history
        .create_channel("archivo".into(), ChannelKind::Text)
        .await
        .unwrap();
    history
        .append_to(
            channel.id.clone(),
            ChatMessage {
                attachments: vec![],
                channel_id: channel.id.clone(),
                id: 7,
                sender_id: 4,
                sender_name: "QA".into(),
                text: "Texto privado del WAL".into(),
                sent_at: 123,
            },
        )
        .await
        .unwrap();
    let archive = root.join("backup.nip.age");
    let exported = backup::export(
        &store,
        archive.clone(),
        SecretString::from("contraseña QA larga".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(exported.messages, 1);
    let raw = std::fs::read(&archive).unwrap();
    assert!(!raw.windows(13).any(|part| part == b"Texto privado"));
    let restored_path = root.join("restored");
    let restored = backup::restore(
        archive,
        restored_path.clone(),
        SecretString::from("contraseña QA larga".to_owned()),
    )
    .await
    .unwrap();
    assert_eq!(restored.messages, 1);
    let restored_store = Store::open(restored_path).unwrap();
    assert_eq!(restored_store.display_name(), "QA backup");
    assert_eq!(
        restored_store.invitation(&id).unwrap().encode().unwrap(),
        invitation
    );
    assert!(
        !restored_store
            .local_server(&id)
            .unwrap()
            .settings
            .auto_start
    );
    assert!(!restored_store.preferences().auto_reconnect);
    assert!(!restored_store.preferences().start_with_windows);
    let restored_history = History::open(restored_store.history_path(&id).unwrap())
        .await
        .unwrap();
    assert!(
        restored_history
            .channels()
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == channel.id)
    );
    assert_eq!(
        restored_history
            .page_in(channel.id, u64::MAX)
            .await
            .unwrap()
            .0[0]
            .text,
        "Texto privado del WAL"
    );
    assert_eq!(
        history
            .page_in("general".into(), u64::MAX)
            .await
            .unwrap()
            .0
            .len(),
        0
    );
    drop(restored_history);
    drop(history);
    std::fs::remove_dir_all(root).unwrap();
}
