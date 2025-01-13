use littlefs2::fs::Filesystem;
use littlefs2::path::{Path, PathBuf};

use heapless::Vec;
use heapless_bytes::Bytes;

use crate::lfs_backup::{BackupBackend, FSBackupError, PathCursor, Result, MAX_FS_DEPTH};

use trussed_core::config::USER_ATTRIBUTE_NUMBER;
use trussed_core::types::UserAttribute;

use std::{
    fs::{remove_file, File},
    io::{Read, Seek as _, SeekFrom, Write},
    path::Path as StdPath,
    path::PathBuf as StdPathBuf,
    string::{String, ToString},
};

pub use generic_array::{
    typenum::{consts, U128, U16, U2, U256, U4096, U512},
    GenericArray,
};

pub struct FileFlash {
    path: std::path::PathBuf,
}

#[derive(Clone)]
struct FileBackend {
    offset: usize,
    path: StdPathBuf,
}

type LfsResult<T> = Result<T, littlefs2::io::Error>;

pub const FS_SIZE: usize = 1920 * 1024; // 2MB - 128kb

const ORIGIN_FS_PATH: &str = "/tmp/test.fs";
const TARGET_FS_PATH: &str = "/tmp/target.fs";
const BACKUP_DATA_PATH: &str = "/tmp/backend.test.bin";

impl FileFlash {
    pub fn new(state_path: impl AsRef<std::path::Path>) -> Self {
        let path: std::path::PathBuf = state_path.as_ref().into();

        if let Ok(file) = File::open(&path) {
            assert_eq!(file.metadata().unwrap().len(), FS_SIZE as u64);
            println!("using existing state file: {path:?}");
        } else {
            let file = File::create(&path).unwrap();
            file.set_len(FS_SIZE as u64).unwrap();
            println!("Created new state file: {path:?}");
        }
        Self { path }
    }
}

impl FileBackend {
    pub fn new(data_path: &StdPath) -> Self {
        let path: std::path::PathBuf = data_path.into();

        const FILE_SIZE: u64 = FS_SIZE as u64;
        if let Ok(file) = File::open(&path) {
            assert_eq!(file.metadata().unwrap().len(), FILE_SIZE);
            println!("using existing backend file: {:?}", &data_path);
        } else {
            let file = File::create(&path).unwrap();
            file.set_len(FILE_SIZE).unwrap();
            println!("Created new backend file: {:?}", &data_path);
        }

        Self { offset: 0, path }
    }
}

impl BackupBackend for FileBackend {
    const RW_SIZE: usize = 256;

    fn write(&mut self, content: &[u8]) -> Result<usize> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|_| FSBackupError::BackendWriteErr)?;

        file.seek(SeekFrom::Start(self.offset as _))
            .map_err(|_| FSBackupError::BackendWriteErr)?;

        let bytes_written = file
            .write(content)
            .map_err(|_| FSBackupError::BackendWriteErr)?;

        assert_eq!(bytes_written, content.len());

        self.offset +=
            bytes_written + ((Self::RW_SIZE - (bytes_written % Self::RW_SIZE)) % Self::RW_SIZE);

        assert_eq!(self.offset % Self::RW_SIZE, 0);

        file.flush().unwrap();
        Ok(bytes_written)
    }

    fn read<const N: usize>(&mut self, len: usize) -> Result<Bytes<N>> {
        let mut buffer = [0u8; N];
        let mut file = File::open(&self.path).map_err(|_| FSBackupError::BackendReadErr)?;

        file.seek(SeekFrom::Start(self.offset as _))
            .map_err(|_| FSBackupError::BackendReadErr)?;

        file.read_exact(&mut buffer[..len])
            .map_err(|_| FSBackupError::BackendReadErr)?;

        let output =
            Bytes::<N>::from_slice(&buffer[..len]).map_err(|_| FSBackupError::BackendReadErr)?;

        assert_eq!(len, output.len());

        self.offset +=
            output.len() + ((Self::RW_SIZE - (output.len() % Self::RW_SIZE)) % Self::RW_SIZE);

        assert_eq!(self.offset % Self::RW_SIZE, 0);

        //println!("requested: {len} new offset: {}", self.offset);
        //println!("{:02x?}", output);

        Ok(output)
    }

    fn erase(&mut self) -> Result<usize> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|_| FSBackupError::BackendWriteErr)?;
        let content: [u8; FS_SIZE] = [0x00u8; FS_SIZE];
        file.write(&content)
            .map_err(|_| FSBackupError::BackendWriteErr)
    }

    fn reset(&mut self) {
        self.offset = 0;
    }
}

fn fill_test_file(fs: &Filesystem<FileFlash>, p: &str, data: &str) -> usize {
    let path = Path::from_bytes_with_nul(p.as_bytes()).expect("path-fail");
    let data = data.as_bytes();
    fs.write(path, data).expect("write fail");

    path.to_string().len() + data.len()
}

fn fill_random_test_file(fs: &Filesystem<FileFlash>, p: &str) -> usize {
    let path = Path::from_bytes_with_nul(p.as_bytes()).expect("path-fail");

    let data: String = thread_rng()
        .sample_iter(&Alphanumeric)
        .take(thread_rng().gen_range(0..512))
        .map(char::from)
        .collect();

    fs.write(path, &data.as_bytes()).expect("write fail");

    path.to_string().len() + data.len()
}

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};

fn fill_test_data(
    fs: &Filesystem<FileFlash>,
    num_files: u32,
    deterministic: bool,
) -> (usize, usize, usize) {
    let mut d_cnt: usize = 0;
    let mut f_cnt: usize = 0;
    let mut data_size = 0;

    let path = PathBuf::from("/was/geht/denn/bluba");
    let data = "blablblalbalab".as_bytes();
    fs.create_dir_all(&path.parent().unwrap())
        .expect("dir create 1 fail");
    fs.write(&path, data).expect("write 1 fail");
    f_cnt += 1;
    d_cnt += 3;
    data_size += data.len() + path.to_string().len();

    let path = PathBuf::from("/was/geht/denn/hier");
    let data = "".as_bytes();
    fs.create_dir_all(&path.parent().unwrap())
        .expect("dir create 2 fail");
    fs.write(&path, data).expect("write 2 fail");
    f_cnt += 1;
    d_cnt += 0;
    data_size += data.len() + &path.to_string().len();

    let path = PathBuf::from("/was/dort");
    let data = "".as_bytes();
    fs.create_dir_all(&path.parent().unwrap())
        .expect("dir create 3 fail");
    fs.write(&path, data).expect("write 3 fail");
    f_cnt += 1;
    d_cnt += 0;
    data_size += data.len() + path.to_string().len();

    let path = PathBuf::from("/testdir/");
    fs.create_dir_all(&path).expect("dir create fail");

    let data =
        "sowqxxxxxxxxxxxxxxxxxxxxxxxxxxxxxdwfqefewefwfwefeweffwewdqwdqdwfewefwefxwefxoejfofwe";

    for idx in 0..num_files {
        if deterministic {
            let data = if idx % 13 != 0 {
                format!("{data}{idx:0>4}")
            } else {
                String::from("")
            };
            data_size += fill_test_file(fs, &format!("/testdir/testfile{idx:0>4}\0"), &data);
        } else {
            data_size += fill_random_test_file(fs, &format!("/testdir/testfile{idx:0>4}\0"));
        }
    }
    d_cnt += 1;
    f_cnt += num_files as usize;

    (d_cnt, f_cnt, data_size)
}

impl littlefs2::driver::Storage for FileFlash {
    const READ_SIZE: usize = 4;
    const WRITE_SIZE: usize = 4;
    const BLOCK_SIZE: usize = 256;

    const BLOCK_COUNT: usize = (FS_SIZE / Self::BLOCK_SIZE as usize);
    const BLOCK_CYCLES: isize = -1;

    type CACHE_SIZE = U256;
    type LOOKAHEADWORDS_SIZE = U2;

    fn read(&mut self, offset: usize, buffer: &mut [u8]) -> LfsResult<usize> {
        let mut file = File::open(&self.path).unwrap();
        file.seek(SeekFrom::Start(offset as _)).unwrap();
        let bytes_read = file.read(buffer).unwrap();
        assert_eq!(bytes_read, buffer.len());
        Ok(bytes_read as _)
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> LfsResult<usize> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .unwrap();
        file.seek(SeekFrom::Start(offset as _)).unwrap();
        let bytes_written = file.write(data).unwrap();
        assert_eq!(bytes_written, data.len());
        file.flush().unwrap();
        Ok(bytes_written)
    }

    fn erase(&mut self, offset: usize, len: usize) -> LfsResult<usize> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&self.path)
            .unwrap();
        file.seek(SeekFrom::Start(offset as _)).unwrap();
        let zero_block = [0xFFu8; Self::BLOCK_SIZE];
        for _ in 0..(len / Self::BLOCK_SIZE) {
            let bytes_written = file.write(&zero_block).unwrap();
            assert_eq!(bytes_written, Self::BLOCK_SIZE);
        }
        file.flush().unwrap();
        Ok(len)
    }
}

pub fn equal_filesystems(
    fs1: &Filesystem<FileFlash>,
    fs2: &Filesystem<FileFlash>,
) -> (usize, usize) {
    let root_dir = PathBuf::from("/");

    let mut path_stack: Vec<PathCursor, MAX_FS_DEPTH> = Vec::new();
    path_stack
        .push(PathCursor {
            path: root_dir,
            idx: 0,
            attr: None,
        })
        .map_err(|_| FSBackupError::PathStackFullErr)
        .unwrap();

    let mut d_cnt: usize = 0;
    let mut f_cnt: usize = 0;

    while !path_stack.is_empty() {
        let mut current = path_stack.pop().unwrap();
        let next_path = FileBackend::get_next_entry(fs1, &current.path, current.idx).unwrap();

        // 'None' => no next item inside this directory, continue w/o adding 'current'
        // back to 'path_stack' implicitly means this subtree is done
        if next_path == None {
            continue;
        }

        let entry = next_path.unwrap();

        // move index "pointer" to next item
        current.idx += 1;

        let info = fs2
            .metadata(entry.path())
            .expect("path (metadata) not found");

        assert_eq!(info, entry.metadata());

        let attr1 = fs1
            .attribute(entry.path(), USER_ATTRIBUTE_NUMBER)
            .unwrap()
            .map(|v| UserAttribute::from_slice(v.data()))
            .transpose()
            .expect("user attr err");

        let attr2 = fs2
            .attribute(entry.path(), USER_ATTRIBUTE_NUMBER)
            .unwrap()
            .map(|v| UserAttribute::from_slice(v.data()))
            .transpose()
            .expect("user attr err");

        assert_eq!(attr1, attr2);

        if entry.metadata().is_file() {
            f_cnt += 1;
            let content1 = fs1.read::<1024>(entry.path()).unwrap();
            let content2 = fs2.read::<1024>(entry.path()).unwrap();
            assert_eq!(content1, content2);

            path_stack.push(current).expect("path stack full");
        } else {
            d_cnt += 1;
            path_stack.push(current).expect("path stack full");

            let mut path = PathBuf::new();
            path.push(entry.path());

            path_stack
                .push(PathCursor {
                    path,
                    idx: 0,
                    attr: attr1,
                })
                .expect("path stack full");
        }
    }
    (d_cnt, f_cnt)
}

fn fsbackup(num_files: u32, deterministic: bool) {
    // cleanup old paths
    for del_path in [ORIGIN_FS_PATH, TARGET_FS_PATH, BACKUP_DATA_PATH].iter() {
        if StdPath::new(del_path).exists() {
            remove_file(del_path).expect("failed deleting file");
        }
    }

    // prepare origin fs (backup target)
    let mut alloc: littlefs2::fs::Allocation<FileFlash> = Filesystem::allocate();
    let mut storage = FileFlash::new(ORIGIN_FS_PATH);
    Filesystem::format(&mut storage).expect("(origin) format failed");
    let fs = Filesystem::mount(&mut alloc, &mut storage).expect("failed mount");

    // output some fs info
    let i_blocks = fs.available_blocks().unwrap();
    let i_space = fs.available_space().unwrap();
    println!(
        "initial - fs blocks: {:?} fs space: {:?}",
        i_blocks, i_space
    );

    // populate origin fs with test data
    let (num_dirs, num_files, data_size) = fill_test_data(&fs, num_files, deterministic);

    // output some fs info (after test data population)
    let f_blocks = fs.available_blocks().unwrap();
    let f_space = fs.available_space().unwrap();
    println!(
        "filled - fs blocks: {:?} fs space: {:?}",
        f_blocks,
        fs.available_space().unwrap()
    );
    let used_blocks = i_blocks - f_blocks;
    let used_space = i_space - f_space;
    let per_file = data_size / num_files;
    let space_usage = (data_size * 100) / used_space;
    println!("used blocks: {used_blocks} used space: {used_space} usage ratio: {space_usage}%");
    println!("wrote dirs/files: {num_dirs}/{num_files} wrote bytes: {data_size} size per file: {per_file}");

    // prepare target fs (restore target)
    let mut target_alloc: littlefs2::fs::Allocation<FileFlash> = Filesystem::allocate();
    let mut target_storage = FileFlash::new(TARGET_FS_PATH);
    Filesystem::format(&mut target_storage).expect("(target) formatting failed");
    let target_fs =
        Filesystem::mount(&mut target_alloc, &mut target_storage).expect("failed target mount");

    // prepare intermediate `FSBackup` storage blob
    let backend_path = StdPath::new(BACKUP_DATA_PATH);
    let mut backend = FileBackend::new(backend_path);

    // execute backup from origin fs -> `FSBackup` storage blob
    let res_backup = backend.backup(&fs);

    backend.reset();

    // execute restore from `FSBackup` storage blob -> target fs
    let res_restore = backend.restore(&target_fs);

    println!("RESULTS: {res_backup:?} {res_restore:?}");

    let count_orig = if let (Ok(b_count), Ok(r_count)) = (res_backup, res_restore) {
        println!("backup count: {b_count:?} restore count: {r_count:?}");
        assert_eq!(b_count, r_count);
        b_count
    } else {
        panic!("failed backup/restore");
    };

    // output some target fs info (restore target)
    println!(
        "restored - fs blocks: {:?} fs space: {:?}",
        target_fs.available_blocks().unwrap(),
        target_fs.available_space().unwrap()
    );

    // check pair-wise equivalence of origin fs vs. target fs
    let count1 = equal_filesystems(&fs, &target_fs);
    let count2 = equal_filesystems(&target_fs, &fs);

    assert_eq!(count1, count2);
    assert_eq!(count1, count_orig);
    assert_eq!((num_dirs, num_files), count_orig);

    println!("filesystems are equal!");
    println!("checked files for equality: {count1:?}");
}

use serial_test::serial;

#[test]
#[serial]
fn fsbackup_deterministic_small() {
    fsbackup(100, true);
}

#[test]
#[serial]
fn fsbackup_nondeterministic_small() {
    fsbackup(100, false);
}

#[test]
#[serial]
fn fsbackup_deterministic_medium() {
    fsbackup(300, true);
}

#[test]
#[serial]
fn fsbackup_nondeterministic_medium() {
    fsbackup(300, false);
}

#[test]
#[serial]
fn fsbackup_nondeterministic_big() {
    fsbackup(750, false);
}
