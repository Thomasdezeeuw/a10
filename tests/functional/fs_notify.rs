use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::Poll;

use a10::fs;
use a10::fs::notify::{Event, Interest, Recursive};

use crate::util::{
    cancel_all, defer, is_send, is_sync, next, poll_nop, remove_test_dir, require_kernel,
    test_queue, tmp_path, Waker,
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

/*
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
            Ok(dir)
        },
        |dir| {
            let path = dir.join(FILE_NAME);
            std::fs::write(&path, DATA)?;
            Ok((path, ()))
        },
        |path, _| {
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
            Ok(dir)
        },
        |dir| {
            let path = dir.join(DIR_NAME);
            std::fs::create_dir(&path)?;
            Ok((path, ()))
        },
        |path, _| {
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
            std::fs::write(&path, DATA).expect("failed to create file");
            watcher.watch_directory(dir.clone(), Interest::OPEN, Recursive::No)?;
            Ok(path)
        },
        |path| {
            let file = std::fs::File::open(&path)?;
            Ok((path, file))
        },
        |path, _| {
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
            Ok(path)
        },
        |path| {
            let dir = std::fs::read_dir(&path)?;
            Ok((path, dir))
        },
        |path, _| {
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
*/

/* TODO.
#[test]
fn watched_directory_file_accessed() {
    test_fs_watcher(
        |watcher, dir| {
            let path = dir.join(FILE_NAME);
            let mut file = std::fs::File::create_new(&path)?;
            dbg!(&file);
            std::io::Write::write(&mut file, DATA)?;
            std::io::Seek::rewind(&mut file)?;
            file.sync_all()?;
            watcher.watch_directory(dir.clone(), Interest::OPEN, Recursive::No)?;
            Ok((path, file))
        },
        |path, mut file| {
            dbg!(&file);
            let mut buf = vec![0; 128];
            let n = std::io::Read::read(&mut file, &mut buf)?;
            assert_eq!(n, DATA.len());
            Ok((path, ()))
        },
        |path, _| {
            vec![ExpectEvent {
                full_path: path.clone(),
                file_path: FILE_NAME,
                accessed: true,
                ..Default::default()
            }]
        },
    );
}
*/

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
        dbg!(event, expected);
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

/*
use crate::util::temp_dir;

#[test]
fn watched_directory_dir_accessed() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_accessed",
            |dir| {
                let path = dir.join(DIR_NAME);
                std::fs::create_dir(&path).expect("failed to create directory");
                let file_path = path.join("file.txt");
                std::fs::write(&file_path, DATA).expect("failed to create file");
                (dir, Interest::ACCESS, Some(Recursive::No), path)
            },
            |path| {
                let mut dir = std::fs::read_dir(&path).expect("failed to open directory");
                let _ = dir
                    .next()
                    .expect("missing file")
                    .expect("failed to read dir");
                (dir, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    accessed: true,
                    ..Default::default()
                }]
            },
            drop, // Close directory and drop path.
        ),
    );
}

#[test]
fn watched_directory_file_modified() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_modified",
            |dir| {
                let path = dir.join(FILE_NAME);
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                let state = (file, path);
                (dir.clone(), Interest::MODIFY, Some(Recursive::No), state)
            },
            |(mut file, path)| {
                std::io::Write::write(&mut file, b"\nHello, again!").expect("failed to write");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    modified: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

// NOTE: `IN_MODIFY` doesn't trigger for directory within a watched directory.

#[test]
fn watched_directory_file_metadata_changed() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_metadata_changed",
            |dir| {
                let path = dir.join(FILE_NAME);
                std::fs::write(&path, DATA).expect("failed to create file");
                (dir, Interest::METADATA, Some(Recursive::No), path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                let metadata = file.metadata().expect("failed to get metadata");
                let mut permissions = metadata.permissions();
                permissions.set_readonly(!permissions.readonly());
                file.set_permissions(permissions)
                    .expect("failed to set permissions");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    metadata_changed: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

#[test]
fn watched_directory_dir_metadata_changed() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_metadata_changed",
            |dir| {
                let path = dir.join(DIR_NAME);
                std::fs::create_dir(&path).expect("failed to create directory");
                let file_path = path.join("file.txt");
                std::fs::write(&file_path, DATA).expect("failed to create file");
                (dir, Interest::METADATA, Some(Recursive::No), path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                let metadata = file.metadata().expect("failed to get metadata");
                let mut permissions = metadata.permissions();
                permissions.set_readonly(!permissions.readonly());
                file.set_permissions(permissions.clone())
                    .expect("failed to set permissions");
                // Reverse it outwise the directory can't be deleted.
                permissions.set_readonly(!permissions.readonly());
                file.set_permissions(permissions)
                    .expect("failed to set permissions");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    metadata_changed: true,
                    ..Default::default()
                }]
            },
            drop, // Close directory and drop path.
        ),
    );
}

#[test]
fn watched_directory_file_moved_from() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_moved_from",
            |dir| {
                let path = dir.join(FILE_NAME);
                std::fs::write(&path, DATA).expect("failed to create file");
                (dir, Interest::MOVE_FROM, Some(Recursive::No), path)
            },
            |path| {
                let to = path
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("watched_directory_file_moved_from.txt");
                std::fs::rename(&path, to).expect("failed to move file");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    file_moved_from: true,
                    file_moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_dir_moved_from() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_moved_from",
            |dir| {
                let path = dir.join(DIR_NAME);
                std::fs::create_dir(&path).expect("failed to create directory");
                (dir, Interest::MOVE_FROM, Some(Recursive::No), path)
            },
            |path| {
                let to = path
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("watched_directory_dir_moved_from.moved");
                std::fs::rename(&path, to).expect("failed to move directory");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    file_moved_from: true,
                    file_moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_file_moved_to() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_moved_to",
            |dir| {
                let from = dir
                    .parent()
                    .unwrap()
                    .join("watched_directory_file_moved_to.txt");
                std::fs::write(&from, DATA).expect("failed to create file");
                let to = dir.join(FILE_NAME);
                (dir, Interest::MOVE_INTO, Some(Recursive::No), (from, to))
            },
            |(from, to)| {
                std::fs::rename(from, &to).expect("failed to move file");
                to
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    file_moved_into: true,
                    file_moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_dir_moved_to() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_moved_to",
            |dir| {
                let from = dir
                    .parent()
                    .unwrap()
                    .join("watched_directory_dir_moved_to.moved");
                std::fs::create_dir(&from).expect("failed to create directory");
                let to = dir.join(DIR_NAME);
                (dir, Interest::MOVE_INTO, Some(Recursive::No), (from, to))
            },
            |(from, to)| {
                std::fs::rename(from, &to).expect("failed to move directory");
                to
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    file_moved_into: true,
                    file_moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_moved() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_moved",
            |dir| (dir.clone(), Interest::MOVE_SELF, Some(Recursive::No), dir),
            |dir| {
                let to = dir.parent().unwrap().join("watched_directory_moved_new");
                std::fs::rename(&dir, to).expect("failed to move directory");
                dir
            },
            |dir| {
                vec![ExpectEvent {
                    full_path: dir.clone(),
                    moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_file_closed_no_write() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_closed_no_write",
            |dir| {
                let path = dir.join(FILE_NAME);
                std::fs::write(&path, DATA).expect("failed to create file");
                (dir, Interest::CLOSE_NOWRITE, Some(Recursive::No), path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    closed_no_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_dir_closed_no_write() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_closed_no_write",
            |dir| {
                let path = dir.join(DIR_NAME);
                std::fs::create_dir(&path).expect("failed to create directory");
                (dir, Interest::CLOSE_NOWRITE, Some(Recursive::No), path)
            },
            |path| {
                let dir = std::fs::read_dir(&path).expect("failed to open directory");
                for entry in dir {
                    entry.expect("failed to read entry");
                }
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    closed_no_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_file_closed_write() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_closed_write",
            |dir| {
                let path = dir.join(FILE_NAME);
                std::fs::write(&path, DATA).expect("failed to create file");
                (dir, Interest::CLOSE_WRITE, Some(Recursive::No), path)
            },
            |path| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    closed_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

// NOTE: no close_write event can be generated for directories.

#[test]
fn watched_directory_dir_deleted() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_deleted",
            |dir| {
                let path = dir.join(DIR_NAME);
                std::fs::create_dir(&path).expect("failed to create directory");
                (dir, Interest::DELETE, Some(Recursive::No), path)
            },
            |path| {
                std::fs::remove_dir(&path).expect("failed to delete directory");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: DIR_NAME,
                    is_dir: true,
                    file_deleted: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_file_deleted() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_file_deleted",
            |dir| {
                let path = dir.join(FILE_NAME);
                std::fs::write(&path, DATA).expect("failed to create file");
                (dir, Interest::DELETE, Some(Recursive::No), path)
            },
            |path| {
                std::fs::remove_file(&path).expect("failed to delete file");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    file_path: FILE_NAME,
                    file_deleted: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_directory_deleted() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_directory_dir_deleted",
            |dir| (dir.clone(), Interest::DELETE_SELF, Some(Recursive::No), dir),
            |path| {
                std::fs::remove_dir(&path).expect("failed to delete direcory");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    deleted: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_opened() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_opened",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::OPEN, None, path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    opened: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

#[test]
fn watched_file_accessed() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_accessed",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::ACCESS, None, path)
            },
            |path| {
                let mut file = std::fs::File::open(&path).expect("failed to open file");
                let mut buf = vec![0; 32];
                let _ = std::io::Read::read(&mut file, &mut buf).expect("failed to read");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    accessed: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

#[test]
fn watched_file_modified() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_modified",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::MODIFY, None, path)
            },
            |path| {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                std::io::Write::write(&mut file, b"\nHello, again!").expect("failed to write");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    modified: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

#[test]
fn watched_file_metadata_changed() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_metadata_change",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::METADATA, None, path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                let metadata = file.metadata().expect("failed to get metadata");
                let mut permissions = metadata.permissions();
                permissions.set_readonly(!permissions.readonly());
                file.set_permissions(permissions)
                    .expect("failed to set permissions");
                (file, path)
            },
            |(_, path)| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    metadata_changed: true,
                    ..Default::default()
                }]
            },
            drop, // Close file and drop path.
        ),
    );
}

#[test]
fn watched_file_moved() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_moved",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::MOVE_SELF, None, path)
            },
            |path| {
                let to = path.parent().unwrap().join("renamed.txt");
                std::fs::rename(&path, &to).expect("failed to rename file");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    moved: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_closed_nowrite() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_closed_nowrite",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::CLOSE_NOWRITE, None, path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    closed_no_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_closed_write() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_closed_write",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::CLOSE_WRITE, None, path)
            },
            |path| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    closed_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_closed_after_read() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_closed_after_read",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::CLOSE, None, path)
            },
            |path| {
                let file = std::fs::File::open(&path).expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    closed_no_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_closed_after_write() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_closed_after_write",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::CLOSE, None, path)
            },
            |path| {
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                drop(file);
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    closed_write: true,
                    closed: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_deleted() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_deleted",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::DELETE_SELF, None, path)
            },
            |path| {
                std::fs::remove_file(&path).expect("failed to delete file");
                path
            },
            |path| {
                vec![ExpectEvent {
                    full_path: path.clone(),
                    deleted: true,
                    ..Default::default()
                }]
            },
            drop, // Drop path.
        ),
    );
}

#[test]
fn watched_file_all() {
    block_on_local_actor(
        actor_fn(actor),
        (
            "fs_watch.watched_file_deleted",
            |tmp_dir| {
                let path = tmp_dir.join(FILE_NAME);
                std::fs::write(path.clone(), DATA).expect("failed to create file");
                (path.clone(), Interest::ALL, None, path)
            },
            |path| {
                // Open.
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("failed to open file");
                // Modified.
                std::io::Write::write(&mut file, b"\nHello, again!").expect("failed to write");
                // Metadata.
                let metadata = file.metadata().expect("failed to get metadata");
                let mut permissions = metadata.permissions();
                permissions.set_readonly(!permissions.readonly());
                file.set_permissions(permissions)
                    .expect("failed to set permissions");
                // Closed (write).
                drop(file);
                // Moved.
                let to = path.parent().unwrap().join("renamed.txt");
                std::fs::rename(&path, &to).expect("failed to rename file");
                path
            },
            |path| {
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
            drop, // Drop path.
        ),
    );
}
*/
