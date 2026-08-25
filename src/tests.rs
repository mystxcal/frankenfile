use std::{
    fs,
    io::Read,
    os::unix::fs::symlink,
    path::Path,
    sync::{Arc, Barrier},
    time::Duration,
};

use tempfile::TempDir;

use crate::{
    archive::{self, ArchiveScope},
    db::Database,
    model::{EntryKind, RedeemResult},
    security::{CodeStyle, MasterKey, normalize_code, random_code, random_session},
    storage::{CreateOptions, ReissueOutcome, Storage},
};

struct Fixture {
    _temp: TempDir,
    source: std::path::PathBuf,
    storage: Storage,
    database: Database,
    key: MasterKey,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("payload");
    fs::create_dir_all(source.join("nested/empty")).unwrap();
    fs::write(source.join("nested/hello.txt"), b"immutable hello\n").unwrap();
    fs::write(source.join("root.bin"), [0u8, 1, 2, 3, 255]).unwrap();
    let storage = Storage::new(temp.path().join("state"));
    storage.prepare().unwrap();
    let database = Database::new(storage.database_path());
    database.initialize().unwrap();
    let key = MasterKey::load_or_create(&storage.master_key_path()).unwrap();
    Fixture {
        _temp: temp,
        source,
        storage,
        database,
        key,
    }
}

fn options() -> CreateOptions {
    CreateOptions {
        title: Some("Test Artifact".to_string()),
        code_ttl: Duration::from_secs(15 * 60),
        drop_ttl: Duration::from_secs(24 * 60 * 60),
        max_redemptions: None,
        public_url: "http://127.0.0.1/frankenfile".to_string(),
        code_style: CodeStyle::Alphanumeric,
    }
}

#[test]
fn reissuing_mints_a_new_code_and_retires_the_old_one() {
    let fx = fixture();
    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let now = created.created_at;

    // A unique 10-character prefix (what the receipt page shows) is accepted.
    let prefix: String = created.drop_id.chars().take(10).collect();
    let outcome = fx
        .storage
        .reissue_code(
            &fx.database,
            &fx.key,
            &prefix,
            Duration::from_secs(3600),
            CodeStyle::Digits,
            "http://127.0.0.1/frankenfile",
        )
        .unwrap();
    let ReissueOutcome::Reissued(reissued) = outcome else {
        panic!("expected a reissued code");
    };
    assert_eq!(reissued.drop_id, created.drop_id);
    assert_ne!(reissued.code, created.code);
    assert!(reissued.code.bytes().all(|b| b.is_ascii_digit()));

    // The old code is dead; the new one redeems.
    let source_tag = fx.key.source_tag("198.51.100.9");
    let old = fx
        .database
        .redeem(
            &fx.key.code_tag(&created.code),
            &source_tag,
            &fx.key.session_tag(&random_session().unwrap()),
            now,
            now + 3600,
            10,
            5,
        )
        .unwrap();
    assert!(matches!(old, RedeemResult::Rejected));
    let new = fx
        .database
        .redeem(
            &fx.key.code_tag(&reissued.code),
            &source_tag,
            &fx.key.session_tag(&random_session().unwrap()),
            now + 61,
            now + 3600,
            10,
            5,
        )
        .unwrap();
    assert!(matches!(new, RedeemResult::Success { .. }));

    // Unknown and too-short references are refused.
    assert!(matches!(
        fx.storage
            .reissue_code(
                &fx.database,
                &fx.key,
                "zzzzzzzzzzzz",
                Duration::from_secs(3600),
                CodeStyle::Alphanumeric,
                "http://127.0.0.1/frankenfile",
            )
            .unwrap(),
        ReissueOutcome::NotFound
    ));
    assert!(
        fx.storage
            .reissue_code(
                &fx.database,
                &fx.key,
                "abc",
                Duration::from_secs(3600),
                CodeStyle::Alphanumeric,
                "http://127.0.0.1/frankenfile",
            )
            .is_err()
    );
}

#[test]
fn codes_are_six_characters_in_the_requested_alphabet() {
    for _ in 0..64 {
        let alnum = random_code(CodeStyle::Alphanumeric).unwrap();
        assert_eq!(alnum.len(), 6);
        assert!(
            alnum
                .bytes()
                .all(|b| b"ABCDEFGHJKMNPQRSTVWXYZ23456789".contains(&b)),
            "unexpected character in {alnum}"
        );
        let digits = random_code(CodeStyle::Digits).unwrap();
        assert_eq!(digits.len(), 6);
        assert!(digits.bytes().all(|b| b.is_ascii_digit()));
    }
}

#[test]
fn submitted_codes_normalize_case_separators_and_reject_garbage() {
    assert_eq!(normalize_code(" ab7-k2q "), Some("AB7K2Q".to_string()));
    assert_eq!(normalize_code("483 921"), Some("483921".to_string()));
    assert_eq!(normalize_code("483921"), Some("483921".to_string()));
    assert_eq!(normalize_code("AB7K2Q"), Some("AB7K2Q".to_string()));
    assert_eq!(normalize_code("abcde"), None);
    assert_eq!(normalize_code("abcdefg"), None);
    assert_eq!(normalize_code("ab7k2!"), None);
    assert_eq!(normalize_code(""), None);
}

#[test]
fn snapshot_refuses_links_and_survives_source_mutation() {
    let fx = fixture();
    symlink("/etc/passwd", fx.source.join("00-outside-link")).unwrap();
    let error = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("symlink"));
    assert!(
        fx.database
            .list_drops(true, time::OffsetDateTime::now_utc().unix_timestamp())
            .unwrap()
            .is_empty()
    );
    fs::remove_file(fx.source.join("00-outside-link")).unwrap();

    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let detail = fx.database.require_drop(&created.drop_id).unwrap();
    assert_eq!(detail.drop.file_count, 2);
    assert!(detail.entries.iter().any(|entry| {
        entry.path == "payload/nested/empty" && entry.kind == EntryKind::Directory
    }));
    let captured = detail
        .entries
        .iter()
        .find(|entry| entry.path == "payload/nested/hello.txt")
        .unwrap();
    fs::write(
        fx.source.join("nested/hello.txt"),
        b"source changed after publish\n",
    )
    .unwrap();
    fs::remove_file(fx.source.join("root.bin")).unwrap();
    let object = fx
        .storage
        .object_path(captured.object_hash.as_deref().unwrap())
        .unwrap();
    assert_eq!(fs::read(object).unwrap(), b"immutable hello\n");
    assert!(fx.storage.doctor(&fx.database, true).unwrap().healthy);
}

#[test]
fn codes_exchange_to_tagged_sessions_and_limits_persist() {
    let fx = fixture();
    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let now = created.created_at;
    let wrong_tag = fx.key.code_tag("999999");
    let source_tag = fx.key.source_tag("198.51.100.4");
    for _ in 0..2 {
        let token = random_session().unwrap();
        let result = fx
            .database
            .redeem(
                &wrong_tag,
                &source_tag,
                &fx.key.session_tag(&token),
                now,
                now + 3600,
                2,
                2,
            )
            .unwrap();
        assert!(matches!(result, RedeemResult::Rejected));
    }

    let token_during_limit = random_session().unwrap();
    assert!(matches!(
        fx.database
            .redeem(
                &fx.key.code_tag(&created.code),
                &source_tag,
                &fx.key.session_tag(&token_during_limit),
                now,
                now + 3600,
                2,
                2,
            )
            .unwrap(),
        RedeemResult::Rejected
    ));

    let token = random_session().unwrap();
    let token_tag = fx.key.session_tag(&token);
    assert!(matches!(
        fx.database
            .redeem(
                &fx.key.code_tag(&created.code),
                &source_tag,
                &token_tag,
                now + 61,
                now + 3600,
                2,
                2,
            )
            .unwrap(),
        RedeemResult::Success { .. }
    ));
    assert!(
        fx.database
            .validate_session(&token_tag, &created.drop_id, now + 62)
            .unwrap()
    );
    let reopened = Database::new(fx.storage.database_path());
    assert!(
        reopened
            .validate_session(&token_tag, &created.drop_id, now + 62)
            .unwrap()
    );
    assert!(fx.database.revoke_drop(&created.drop_id, now + 63).unwrap());
    assert!(
        !fx.database
            .validate_session(&token_tag, &created.drop_id, now + 64)
            .unwrap()
    );

    assert!(!tree_contains_bytes(
        fx.storage.root_for_test(),
        created.code.as_bytes()
    ));
    assert!(!tree_contains_bytes(
        fx.storage.root_for_test(),
        token.as_bytes()
    ));
}

#[test]
fn concurrent_failures_are_atomically_bounded_by_the_global_budget() {
    let fx = fixture();
    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let now = created.created_at;
    let workers = 32;
    let barrier = Arc::new(Barrier::new(workers));
    let mut threads = Vec::new();
    for index in 0..workers {
        let database = fx.database.clone();
        let key = fx.key.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let source = key.source_tag(&format!("198.51.100.{index}"));
            let session = key.session_tag(&random_session().unwrap());
            barrier.wait();
            database
                .redeem(
                    &key.code_tag("000000"),
                    &source,
                    &session,
                    now,
                    now + 3600,
                    10,
                    5,
                )
                .unwrap()
        }));
    }
    for thread in threads {
        assert!(matches!(thread.join().unwrap(), RedeemResult::Rejected));
    }
    let connection = rusqlite::Connection::open(fx.storage.database_path()).unwrap();
    let failures: i64 = connection
        .query_row("SELECT COUNT(*) FROM redemption_failures", [], |row| {
            row.get(0)
        })
        .unwrap();
    let throttle_events: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE event='redeem_throttled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failures, 10);
    assert_eq!(throttle_events, 1);
}

#[test]
fn deterministic_archives_preserve_paths_empty_dirs_and_bytes() {
    let fx = fixture();
    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let detail = fx.database.require_drop(&created.drop_id).unwrap();
    let first =
        archive::materialize(&fx.storage, &fx.database, &detail, &ArchiveScope::Whole).unwrap();
    let first_path = fx.storage.archive_path(&first.relative_path).unwrap();
    let first_bytes = fs::read(&first_path).unwrap();
    fs::remove_file(&first_path).unwrap();
    fx.database.remove_archive_record(&first.cache_key).unwrap();
    let second =
        archive::materialize(&fx.storage, &fx.database, &detail, &ArchiveScope::Whole).unwrap();
    let second_bytes = fs::read(fx.storage.archive_path(&second.relative_path).unwrap()).unwrap();
    assert_eq!(first_bytes, second_bytes);

    let reader = std::io::Cursor::new(second_bytes);
    let mut zip = zip::ZipArchive::new(reader).unwrap();
    let names = (0..zip.len())
        .map(|index| zip.by_index(index).unwrap().name().to_string())
        .collect::<Vec<_>>();
    assert!(names.contains(&"payload/nested/empty/".to_string()));
    let mut hello = String::new();
    zip.by_name("payload/nested/hello.txt")
        .unwrap()
        .read_to_string(&mut hello)
        .unwrap();
    assert_eq!(hello, "immutable hello\n");

    let folder = archive::materialize(
        &fx.storage,
        &fx.database,
        &detail,
        &ArchiveScope::Folder("payload".to_string()),
    )
    .unwrap();
    assert!(
        fx.storage
            .archive_path(&folder.relative_path)
            .unwrap()
            .is_file()
    );
}

#[test]
fn retention_gc_purges_old_drops_and_reclaims_unshared_objects() {
    let fx = fixture();
    let created = fx
        .storage
        .create_drop(
            &fx.database,
            &fx.key,
            std::slice::from_ref(&fx.source),
            &options(),
        )
        .unwrap();
    let future = created.created_at + 10 * 24 * 3600;
    let retention = Duration::from_secs(7 * 24 * 3600);
    let dry = fx
        .storage
        .garbage_collect(&fx.database, false, future, retention)
        .unwrap();
    assert_eq!(dry.purgable_drops, 1);
    assert_eq!(dry.unreachable_objects, 2);
    assert_eq!(dry.deleted_files, 0);
    let applied = fx
        .storage
        .garbage_collect(&fx.database, true, future, retention)
        .unwrap();
    assert_eq!(applied.purged_drops, 1);
    assert_eq!(applied.deleted_files, 2);
    assert!(fx.database.get_drop(&created.drop_id).unwrap().is_none());
    assert!(fx.database.referenced_objects().unwrap().is_empty());
}

fn tree_contains_bytes(root: &Path, needle: &[u8]) -> bool {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        if bytes.windows(needle.len()).any(|window| window == needle) {
            return true;
        }
    }
    false
}
