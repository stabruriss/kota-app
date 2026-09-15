use super::*;
use crate::{
    agent_directory::tests::Account,
    bbs_sync::transport::{Cancellation, FileIo, Limits},
};

#[tokio::test]
async fn registered_image_stamps_avoid_rehash_on_names_and_detect_same_size_replacement() {
    let account = Account::new();
    account.project("p", false);
    for id in ["a", "b"] {
        account.agent("p", id, "display-name: Photo\navatar-id: user:pic\n");
    }
    let dir = account.0.join("avatars");
    fs::create_dir_all(&dir).unwrap();
    let bytes = vec![42; 100_000];
    fs::write(dir.join("pic.png"), &bytes).unwrap();
    fs::write(dir.join("avatars.json"), br#"[{"id":"user:pic","label":"Picture","fileName":"pic.png","mime":"image/png","createdAt":"now"}]"#).unwrap();
    let store = ContentStore::at(account.0.join("Workspaces/bbs"), account.0.clone());
    store.prepare_layout().unwrap();
    let files = FileIo::start(Limits::default()).unwrap();
    files
        .run(&Cancellation::default(), move |io| {
            let mut cache = Images::default();
            let first = cache.collect(&store, io).unwrap();
            assert_eq!(cache.source_reads, 1);
            assert!(cache.available(&store, &first[0].agents[0].avatar, io));
            assert_eq!(cache.blob_hashes, 0);
            let source = store
                .account_dir()
                .join("Workspaces/p/.agent-workspaces/a/agent.yaml");
            fs::write(
                source,
                "display-name: Renamed\navatar-id: user:pic\nsession-id: later\n",
            )
            .unwrap();
            let renamed = cache.collect(&store, io).unwrap();
            assert_eq!(renamed[0].agents[0].name, "Renamed");
            assert_eq!(cache.source_reads, 1);
            assert_eq!(cache.blob_hashes, 0);
            let image = store.account_dir().join("avatars/pic.png");
            let temp = image.with_extension("new");
            fs::write(&temp, vec![43; 100_000]).unwrap();
            fs::rename(temp, &image).unwrap();
            let changed = cache.collect(&store, io).unwrap();
            assert_ne!(changed[0].agents[0].avatar, first[0].agents[0].avatar);
            assert_eq!(cache.source_reads, 2);
            fs::remove_file(image).unwrap();
            let missing = cache.collect(&store, io).unwrap();
            assert_eq!(missing[0].agents.len(), 2);
            assert_eq!(missing[0].agents[0].avatar, Avatar::None);
        })
        .await
        .unwrap();
    files.stop_and_wait().await;
}
