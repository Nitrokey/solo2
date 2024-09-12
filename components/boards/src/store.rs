use core::mem::MaybeUninit;

use apps::InitStatus;
use littlefs2::{
    const_ram_storage,
    driver::Storage,
    driver::Storage as LfsStorage,
    fs::{Allocation, Filesystem},
    io::Result as LfsResult,
};
use trussed::store::{Fs, Store};

use crate::Board;

// 8KB of RAM
const_ram_storage!(
    name = VolatileStorage,
    trait = LfsStorage,
    erase_value = 0xff,
    read_size = 16,
    write_size = 256,
    cache_size_ty = littlefs2::consts::U256,
    // We use 256 instead of the default 512 to avoid loosing too much space to nearly empty blocks containing only folder metadata.
    block_size = 256,
    block_count = 8192/256,
    lookahead_size_ty = littlefs2::consts::U1,
    filename_max_plus_one_ty = littlefs2::consts::U256,
    path_max_plus_one_ty = littlefs2::consts::U256,
    result = LfsResult,
);

pub struct StoreResources<B: Board> {
    initialized: bool,
    ifs: StorageResources<B::InternalStorage>,
    efs: StorageResources<B::ExternalStorage>,
    vfs: StorageResources<VolatileStorage>,
}

impl<B: Board> StoreResources<B> {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            ifs: StorageResources::new(),
            efs: StorageResources::new(),
            vfs: StorageResources::new(),
        }
    }
}

struct StorageResources<S: Storage + 'static> {
    storage: MaybeUninit<(S, Allocation<S>)>,
    fs: MaybeUninit<Filesystem<'static, S>>,
    fs_ptr: MaybeUninit<Fs<S>>,
}

impl<S: Storage + 'static> StorageResources<S> {
    const fn new() -> Self {
        Self {
            storage: MaybeUninit::uninit(),
            fs: MaybeUninit::uninit(),
            fs_ptr: MaybeUninit::uninit(),
        }
    }
}

pub struct RunnerStore<B: Board> {
    ifs: &'static Fs<B::InternalStorage>,
    efs: &'static Fs<B::ExternalStorage>,
    vfs: &'static Fs<VolatileStorage>,
}

impl<B: Board> Clone for RunnerStore<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Board> Copy for RunnerStore<B> {}

unsafe impl<B: Board> Store for RunnerStore<B> {
    type I = B::InternalStorage;
    type E = B::ExternalStorage;
    type V = VolatileStorage;

    fn ifs(self) -> &'static Fs<Self::I> {
        self.ifs
    }

    fn efs(self) -> &'static Fs<Self::E> {
        self.efs
    }

    fn vfs(self) -> &'static Fs<Self::V> {
        self.vfs
    }
}

pub fn init_store<B: Board>(
    resources: &'static mut StoreResources<B>,
    int_flash: B::InternalStorage,
    ext_flash: B::ExternalStorage,
    simulated_efs: bool,
    status: &mut InitStatus,
) -> RunnerStore<B> {
    if resources.initialized {
        panic!("multiple instances of RunnerStore are not allowed");
    }
    resources.initialized = true;

    let ifs_resources = resources
        .ifs
        .storage
        .write((int_flash, Filesystem::allocate()));
    let (ifs_storage, ifs_alloc) = (&mut ifs_resources.0, &mut ifs_resources.1);
    let efs_resources = resources
        .efs
        .storage
        .write((ext_flash, Filesystem::allocate()));
    let (efs_storage, efs_alloc) = (&mut efs_resources.0, &mut efs_resources.1);
    let vfs_resources = resources
        .vfs
        .storage
        .write((VolatileStorage::new(), Filesystem::allocate()));
    let (vfs_storage, vfs_alloc) = (&mut vfs_resources.0, &mut vfs_resources.1);

    let ifs = match init_ifs::<B>(ifs_storage, ifs_alloc, efs_storage, status) {
        Ok(ifs) => resources.ifs.fs.write(ifs),
        Err(_e) => {
            error!("IFS Mount Error {:?}", _e);
            panic!("IFS");
        }
    };

    let efs = match init_efs::<B>(efs_storage, efs_alloc, simulated_efs, status) {
        Ok(efs) => resources.efs.fs.write(efs),
        Err(_e) => {
            error!("EFS Mount Error {:?}", _e);
            panic!("EFS");
        }
    };

    let vfs = match init_vfs(vfs_storage, vfs_alloc) {
        Ok(vfs) => resources.vfs.fs.write(vfs),
        Err(_e) => {
            error!("VFS Mount Error {:?}", _e);
            panic!("VFS");
        }
    };

    let ifs = resources.ifs.fs_ptr.write(Fs::new(ifs));
    let efs = resources.efs.fs_ptr.write(Fs::new(efs));
    let vfs = resources.vfs.fs_ptr.write(Fs::new(vfs));

    RunnerStore { ifs, efs, vfs }
}

#[inline(always)]
fn init_ifs<B: Board>(
    ifs_storage: &'static mut B::InternalStorage,
    ifs_alloc: &'static mut Allocation<B::InternalStorage>,
    efs_storage: &mut B::ExternalStorage,
    status: &mut InitStatus,
) -> LfsResult<Filesystem<'static, B::InternalStorage>> {
    if !Filesystem::is_mountable(ifs_storage) {
        // handle provisioner
        if cfg!(feature = "provisioner") {
            info_now!("IFS mount failed - provisioner => formatting");
            Filesystem::format(ifs_storage).ok();
        } else {
            status.insert(InitStatus::INTERNAL_FLASH_ERROR);
            error_now!("IFS mount-fail");
            B::recover_ifs(ifs_storage, ifs_alloc, efs_storage).ok();
        }
    }

    B::prepare_ifs(ifs_storage);

    Filesystem::mount(ifs_alloc, ifs_storage)
}

#[inline(always)]
fn init_efs<B: Board>(
    efs_storage: &'static mut B::ExternalStorage,
    efs_alloc: &'static mut Allocation<B::ExternalStorage>,
    simulated_efs: bool,
    status: &mut InitStatus,
) -> LfsResult<Filesystem<'static, B::ExternalStorage>> {
    if !Filesystem::is_mountable(efs_storage) {
        let fmt_ext = Filesystem::format(efs_storage);
        if simulated_efs && fmt_ext == Err(littlefs2::io::Error::NoSpace) {
            info_now!("Formatting simulated EFS failed as expected");
        } else {
            error_now!("EFS Mount Error, Reformat {:?}", fmt_ext);
            status.insert(InitStatus::EXTERNAL_FLASH_ERROR);
        }
    };
    Filesystem::mount(efs_alloc, efs_storage)
}

#[inline(always)]
fn init_vfs(
    vfs_storage: &'static mut VolatileStorage,
    vfs_alloc: &'static mut Allocation<VolatileStorage>,
) -> LfsResult<Filesystem<'static, VolatileStorage>> {
    if !Filesystem::is_mountable(vfs_storage) {
        Filesystem::format(vfs_storage).ok();
    }
    Filesystem::mount(vfs_alloc, vfs_storage)
}
