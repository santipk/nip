use nip_native::{contracts::ChatMessage, history::History};

#[tokio::test]
async fn local_channel_catalog_survives_restart_and_preserves_one_text() {
    use nip_native::contracts::ChannelKind;
    let directory =
        std::env::temp_dir().join(format!("nip-channel-catalog-{}", getrandom::u64().unwrap()));
    let path = directory.join("history.sqlite3");
    let history = History::open(path.clone()).await.unwrap();
    assert_eq!(history.channels().await.unwrap().len(), 2);
    assert!(history.delete_channel("general".into()).await.is_err());
    let created = history
        .create_channel("Otro texto".into(), ChannelKind::Text)
        .await
        .unwrap();
    history
        .rename_channel(created.id.clone(), "Conversación".into())
        .await
        .unwrap();
    history.delete_channel("general".into()).await.unwrap();
    drop(history);
    let history = History::open(path).await.unwrap();
    let channels = history.channels().await.unwrap();
    assert!(
        channels
            .iter()
            .any(|c| c.id == created.id && c.name == "Conversación")
    );
    assert!(!channels.iter().any(|c| c.id == "general"));
    drop(history);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn channels_keep_separate_history_after_restart() {
    let directory =
        std::env::temp_dir().join(format!("nip-channel-history-{}", getrandom::u64().unwrap()));
    let path = directory.join("history.sqlite3");
    let history = History::open(path.clone()).await.unwrap();
    history
        .append(ChatMessage {
            attachments: vec![],
            channel_id: "general".into(),
            id: 1,
            sender_id: 1,
            sender_name: "QA".into(),
            text: "anterior".into(),
            sent_at: 1,
        })
        .await
        .unwrap();
    history
        .append_to(
            "other".into(),
            ChatMessage {
                attachments: vec![],
                channel_id: "other".into(),
                id: 2,
                sender_id: 1,
                sender_name: "QA".into(),
                text: "otro canal".into(),
                sent_at: 2,
            },
        )
        .await
        .unwrap();
    drop(history);
    let history = History::open(path).await.unwrap();
    assert_eq!(
        history
            .page(u64::MAX)
            .await
            .unwrap()
            .0
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        vec!["anterior"]
    );
    assert_eq!(
        history
            .page_in("other".into(), u64::MAX)
            .await
            .unwrap()
            .0
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>(),
        vec!["otro canal"]
    );
    assert_eq!(history.counters().await.unwrap(), (3, 2));
    drop(history);
    std::fs::remove_dir_all(directory).unwrap();
}
