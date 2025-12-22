use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::Poll;

use a10::fs;
use a10::fs::notify::{Event, Interest, Recursive};

use crate::util::{
    Waker, defer, is_send, is_sync, next, poll_nop, remove_test_dir, require_kernel, test_queue,
    tmp_file_path, tmp_path,
};

const FILE_NAME: &str = "test.txt";
const DIR_NAME: &str = "some_dir";
const DATA: &[u8] = b"Hello, World!";

#[test]
fn watcher_is_send_and_sync() {
    is_send::<fs::Watcher>();
    is_sync::<fs::Watcher>();
}

#[test]
fn events_is_send_and_sync() {
    is_send::<fs::notify::Events>();
    is_sync::<fs::notify::Events>();
}

#[test]
fn event_is_send_and_sync() {
    is_send::<&fs::notify::Event>();
    is_sync::<&fs::notify::Event>();
}

#[test]
fn watched_directory_file_created() {
    test_fs_watcher(
        |watcher, dir| {
            watcher.watch_directory(dir.clone(), Interest::CREATE, Recursive::No)?;
            Ok((dir, ()))
        },
        |dir, ()| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                file_created: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_created() {
    test_fs_watcher(
        |watcher, dir| {
            watcher.watch_directory(dir.clone(), Interest::CREATE, Recursive::No)?;
            Ok((dir, ()))
        },
        |dir, ()| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                file_created: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_opened() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::OPEN, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                opened: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_opened() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            watcher.watch_directory(dir.clone(), Interest::OPEN, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let dir = std::fs::read_dir(&path)?;
            Ok((path, dir))
        },
        |path, _dir| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                opened: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_accessed() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            let mut file = std::fs::File::create_new(&path)?;
            std::io::Write::write(&mut file, DATA)?;
            std::io::Seek::rewind(&mut file)?;
            file.sync_all()?;
            watcher.watch_directory(dir.clone(), Interest::ACCESS, Recursive::No)?;
            Ok((path, file))
        },
        |path, mut file| {
            let mut buf = vec![0; 128];
            let n = std::io::Read::read(&mut file, &mut buf)?;
            assert_eq!(n, DATA.len());
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                accessed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_accessed() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            let file_path = path.join("file.txt");
            std::fs::write(&file_path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::ACCESS, Recursive::No)?;
            Ok((dir, ()))
        },
        |path, ()| {
            let mut dir = std::fs::read_dir(&path)?;
            let _ = dir.next().expect("missing file")?;
            Ok((path, dir))
        },
        |path, _dir| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: "",
                is_dir: true,
                accessed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_modified() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&path)?;
            watcher.watch_directory(dir.clone(), Interest::MODIFY, Recursive::No)?;
            Ok((path, file))
        },
        |path, mut file| {
            std::io::Write::write(&mut file, b"\nHello, again!")?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                modified: true,
                ..Default::default()
            }]
        },
    );
}

// NOTE: `IN_MODIFY` doesn't trigger for directory within a watched directory.

#[test]
fn watched_directory_file_metadata_changed() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::METADATA, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            let metadata = file.metadata()?;
            let mut permissions = metadata.permissions();
            permissions.set_readonly(!permissions.readonly());
            file.set_permissions(permissions)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                metadata_changed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_metadata_changed() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            let file_path = path.join("file.txt");
            std::fs::write(&file_path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::METADATA, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            let metadata = file.metadata()?;
            let mut permissions = metadata.permissions();
            permissions.set_readonly(!permissions.readonly());
            file.set_permissions(permissions.clone())?;
            // Reverse it outwise the directory can't be deleted.
            permissions.set_readonly(!permissions.readonly());
            file.set_permissions(permissions)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                metadata_changed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_moved_from() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::MOVE_FROM, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let to = path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("watched_directory_file_moved_from.txt");
            std::fs::rename(&path, to)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                file_moved_from: true,
                file_moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_moved_from() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            watcher.watch_directory(dir.clone(), Interest::MOVE_FROM, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let to = path
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("watched_directory_dir_moved_from.moved");
            std::fs::rename(&path, to)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                file_moved_from: true,
                file_moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_moved_to() {
    test_fs_watcher(
        |watcher, dir| {
            let from = tmp_file_path("txt");
            std::fs::write(&from, DATA)?;
            let to = dir.join(FILE_NAME);
            watcher.watch_directory(dir.clone(), Interest::MOVE_INTO, Recursive::No)?;
            Ok((from, to))
        },
        |from, to| {
            std::fs::rename(&from, &to)?;
            Ok((to, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                file_moved_into: true,
                file_moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_moved_to() {
    test_fs_watcher(
        |watcher, dir| {
            let from = dir
                .parent()
                .unwrap()
                .join("watched_directory_dir_moved_to.moved");
            std::fs::create_dir(&from)?;
            let to = dir.join(DIR_NAME);
            watcher.watch_directory(dir.clone(), Interest::MOVE_INTO, Recursive::No)?;
            Ok((from, to))
        },
        |from, to| {
            std::fs::rename(&from, &to)?;
            Ok((to, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                file_moved_into: true,
                file_moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_moved() {
    test_fs_watcher(
        |watcher, dir| {
            watcher.watch_directory(dir.clone(), Interest::MOVE_SELF, Recursive::No)?;
            Ok((dir, ()))
        },
        |path, ()| {
            let to = tmp_file_path("txt");
            std::fs::rename(&path, &to)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_closed_no_write() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::CLOSE_NOWRITE, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                closed_no_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_dir_closed_no_write() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            watcher.watch_directory(dir.clone(), Interest::CLOSE_NOWRITE, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let dir = std::fs::read_dir(&path)?;
            for entry in dir {
                entry?;
            }
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                closed_no_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_closed_write() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::CLOSE_WRITE, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                closed_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

// NOTE: no close_write event can be generated for directories.

#[test]
fn watched_directory_dir_deleted() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            watcher.watch_directory(dir.clone(), Interest::DELETE, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            std::fs::remove_dir(&path)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: DIR_NAME,
                is_dir: true,
                file_deleted: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_file_deleted() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            watcher.watch_directory(dir.clone(), Interest::DELETE, Recursive::No)?;
            Ok((path, ()))
        },
        |path, ()| {
            std::fs::remove_file(&path)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                file_deleted: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_directory_deleted() {
    test_fs_watcher(
        |watcher, dir| {
            watcher.watch_directory(dir.clone(), Interest::DELETE_SELF, Recursive::No)?;
            Ok((dir, ()))
        },
        |path, ()| {
            std::fs::remove_dir(&path)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                deleted: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_opened() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::OPEN)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                opened: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_accessed() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::ACCESS)?;
            Ok((path, ()))
        },
        |path, ()| {
            let mut file = std::fs::File::open(&path)?;
            let mut buf = vec![2; 32];
            let _ = std::io::Read::read(&mut file, &mut buf)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                accessed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_modified() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::MODIFY)?;
            Ok((path, ()))
        },
        |path, ()| {
            let mut file = std::fs::OpenOptions::new().write(true).open(&path)?;
            std::io::Write::write(&mut file, b"\nHello, again!")?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                modified: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_metadata_changed() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::METADATA)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            let metadata = file.metadata()?;
            let mut permissions = metadata.permissions();
            permissions.set_readonly(!permissions.readonly());
            file.set_permissions(permissions)?;
            Ok((path, file))
        },
        |path, _file| {
            vec![ExpectEvent {
                full_path: path.clone(),
                metadata_changed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_moved() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::MOVE_SELF)?;
            Ok((path, ()))
        },
        |path, ()| {
            let to = path.parent().unwrap().join("renamed.txt");
            std::fs::rename(&path, &to)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                moved: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_closed_nowrite() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::CLOSE_NOWRITE)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                closed_no_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_closed_write() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::CLOSE_WRITE)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                closed_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_closed_after_read() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::CLOSE)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::File::open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                closed_no_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_closed_after_write() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::CLOSE)?;
            Ok((path, ()))
        },
        |path, ()| {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            drop(file);
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                closed_write: true,
                closed: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_deleted() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::DELETE_SELF)?;
            Ok((path, ()))
        },
        |path, ()| {
            std::fs::remove_file(&path)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![ExpectEvent {
                full_path: path.clone(),
                deleted: true,
                ..Default::default()
            }]
        },
    );
}

#[test]
fn watched_file_all() {
    test_fs_watcher(
        |watcher, tmp_dir| {
            let path = tmp_dir.join(FILE_NAME);
            std::fs::write(path.clone(), DATA)?;
            watcher.watch_file(path.clone(), Interest::ALL)?;
            Ok((path, ()))
        },
        |path, ()| {
            // Open.
            let mut file = std::fs::OpenOptions::new().write(true).open(&path)?;
            // Modified.
            std::io::Write::write(&mut file, b"\nHello, again!")?;
            // Metadata.
            let metadata = file.metadata()?;
            let mut permissions = metadata.permissions();
            permissions.set_readonly(!permissions.readonly());
            file.set_permissions(permissions)?;
            // Closed (write).
            drop(file);
            // Moved.
            let to = path.parent().unwrap().join("renamed.txt");
            std::fs::rename(&path, &to)?;
            Ok((path, ()))
        },
        |path, ()| {
            vec![
                ExpectEvent {
                    full_path: path.clone(),
                    opened: true,
                    ..Default::default()
                },
                ExpectEvent {
                    full_path: path.clone(),
                    modified: true,
                    ..Default::default()
                },
                ExpectEvent {
                    full_path: path.clone(),
                    metadata_changed: true,
                    ..Default::default()
                },
                ExpectEvent {
                    full_path: path.clone(),
                    closed_write: true,
                    closed: true,
                    ..Default::default()
                },
                ExpectEvent {
                    full_path: path.clone(),
                    moved: true,
                    ..Default::default()
                },
            ]
        },
    );
}

#[test]
fn watch_recursive_not_directory() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            // Recursive should be ignored for watching files.
            watcher.watch(path.clone(), Interest::ALL, Recursive::All)?;
            Ok((path, ()))
        },
        |path, ()| {
            let mut file = std::fs::OpenOptions::new().write(true).open(&path)?;
            std::io::Write::write(&mut file, b"\nHello, again!")?;
            Ok((path, file))
        },
        |path, _file| {
            vec![
                ExpectEvent {
                    full_path: path.clone(),
                    opened: true,
                    ..Default::default()
                },
                ExpectEvent {
                    full_path: path.clone(),
                    modified: true,
                    ..Default::default()
                },
            ]
        },
    );
}

fn test_fs_watcher<T, U>(
    watch: impl Fn(&mut fs::Watcher, PathBuf) -> io::Result<(PathBuf, T)>,
    trigger: impl Fn(PathBuf, T) -> io::Result<(PathBuf, U)>,
    expected: impl Fn(PathBuf, &U) -> Vec<ExpectEvent>,
) {
    require_kernel!(6, 7); // Needed for ReadBufPool.
    let sq = test_queue();
    let waker = Waker::new();

    let mut watcher = fs::Watcher::new(sq.clone()).expect("failed to create watcher");

    let path = tmp_path();
    std::fs::create_dir(&path).expect("failed to create temporary test directory");
    let _d = defer(|| remove_test_dir(&path));

    let (path, resources) = watch(&mut watcher, path.clone()).expect("failed to setup up watches");

    let (path, resources) = trigger(path, resources).expect("failed to trigger fs events");
    let expected = expected(path, &resources);

    waker.block_on(expect_events(&mut watcher, &expected));
    drop(resources);
}

async fn expect_events(watcher: &mut fs::Watcher, expected: &[ExpectEvent]) {
    let mut events = watcher.events();
    for expected in expected {
        let event = next(&mut events)
            .await
            .expect("missing expected event")
            .expect("failed to read events");
        assert_eq!(event, expected);
        let full_path = events.path_for(&event);
        assert_eq!(full_path, expected.full_path);
    }
    if let Poll::Ready(Some(event)) = poll_nop(Pin::new(&mut next(&mut events))) {
        panic!("unexpected event: {event:?}");
    }
}

#[derive(Debug, Default)]
struct ExpectEvent {
    full_path: PathBuf,
    file_path: &'static str,
    is_dir: bool,
    accessed: bool,
    modified: bool,
    metadata_changed: bool,
    closed_write: bool,
    closed_no_write: bool,
    closed: bool,
    opened: bool,
    deleted: bool,
    moved: bool,
    unmounted: bool,
    file_moved_from: bool,
    file_moved_into: bool,
    file_moved: bool,
    file_created: bool,
    file_deleted: bool,
}

impl PartialEq<ExpectEvent> for Event {
    #[rustfmt::skip]
    fn eq(&self, event: &ExpectEvent) -> bool {
        // Don't want to print the entire event as it's quite big, just print
        // the field that differs.
        assert_eq!(self.file_path(), Path::new(event.file_path), "file_path");
        assert_eq!(self.is_dir(), event.is_dir, "is_dir");
        assert_eq!(self.accessed(), event.accessed, "accessed");
        assert_eq!(self.modified(), event.modified, "modified");
        assert_eq!(self.metadata_changed(), event.metadata_changed, "metadata_changed");
        assert_eq!(self.closed_write(), event.closed_write, "closed_write");
        assert_eq!(self.closed_no_write(), event.closed_no_write, "closed_no_write");
        assert_eq!(self.closed(), event.closed, "closed");
        assert_eq!(self.opened(), event.opened, "opened");
        assert_eq!(self.deleted(), event.deleted, "deleted");
        assert_eq!(self.moved(), event.moved, "moved");
        assert_eq!(self.unmounted(), event.unmounted, "unmounted");
        assert_eq!(self.file_moved_from(), event.file_moved_from, "file_moved_from");
        assert_eq!(self.file_moved_into(), event.file_moved_into, "file_moved_into");
        assert_eq!(self.file_moved(), event.file_moved, "file_moved");
        assert_eq!(self.file_created(), event.file_created, "file_created");
        assert_eq!(self.file_deleted(), event.file_deleted, "file_deleted");
        true
    }
}
