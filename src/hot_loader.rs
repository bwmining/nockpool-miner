// hot_loader.rs (côté hôte)
use anyhow::{bail, Result};
use libloading::{Library, Symbol};
use nockvm::jets::hot::HotEntry;
use std::{
    env,
    path::{Path, PathBuf},
    slice,
    sync::{Arc, OnceLock},
}; // même type que dans la lib


type PluginsInitFn = unsafe extern "C" fn(extern "C" fn(u32)) -> bool;
type LenFn = unsafe extern "C" fn() -> usize;
type PtrFn = unsafe extern "C" fn() -> *const HotEntry;
type ApiFn = unsafe extern "C" fn() -> u32;
type CancelFn = unsafe extern "C" fn();

pub struct HotLibrary {
    _lib: Library,        // garde la lib vivante
    ptr: *const HotEntry, // pointeur vers la table dans la lib
    len: usize,
    request_cancel_fn: Option<CancelFn>,
    reset_cancel_fn: Option<CancelFn>,
}

unsafe impl Send for HotLibrary {}
unsafe impl Sync for HotLibrary {}

impl HotLibrary {
    /// Find the library in the binary directory
    pub fn find_library() -> Option<PathBuf> {
        // Try to get the current executable path
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let lib_path = exe_dir.join("libzkvm_jetpack.so");
                if lib_path.exists() {
                    return Some(lib_path);
                }
            }
        }

        // Fallback to current directory
        let current_dir_path = PathBuf::from("libzkvm_jetpack.so");
        if current_dir_path.exists() {
            Some(current_dir_path)
        } else {
            None
        }
    }

    /// Try to load library automatically (binary dir, then current dir)
    pub unsafe fn load_auto(report_callback: Option<extern "C" fn(u32)>) -> Result<Self> {
        if let Some(path) = Self::find_library() {
            Self::load(path, report_callback)
        } else {
            bail!("libzkvm_jetpack.so not found in binary directory or current directory")
        }
    }

    /// Charge la lib dynamique et récupère le slice des jets
    pub unsafe fn load<P: AsRef<Path>>(
        path: P,
        report_callback: Option<extern "C" fn(u32)>,
    ) -> Result<Self> {
        let lib = Library::new(path.as_ref())?;

        // Résout les symboles
        let api: Symbol<ApiFn> = lib.get(b"prover_hot_api_version")?;
        if api() != 1 {
            bail!("Hot API version mismatch");
        }

        let len_sym: Symbol<LenFn> = lib.get(b"prover_hot_state_len")?;
        let ptr_sym: Symbol<PtrFn> = lib.get(b"prover_hot_state_ptr")?;
        let request_cancel_fn = lib
            .get::<CancelFn>(b"nockminer_request_cancel")
            .ok()
            .map(|sym| *sym);
        let reset_cancel_fn = lib
            .get::<CancelFn>(b"nockminer_reset_cancel")
            .ok()
            .map(|sym| *sym);
        if let (Some(init_fn), Some(report)) =
            (lib.get::<PluginsInitFn>(b"nockminer_plugins_init").ok(), report_callback)
        {
            if !unsafe { init_fn(report) } {
                tracing::warn!("nockminer_plugins_init reported failure");
            }
        }

        let len = len_sym();
        let ptr = ptr_sym();

        if ptr.is_null() || len == 0 {
            bail!("Empty or null hot state from dynamic library");
        }

        Ok(Self {
            _lib: lib,
            ptr,
            len,
            request_cancel_fn,
            reset_cancel_fn,
        })
    }

    /// Vue sur la table des jets (vivante tant que `self` vit)
    pub fn jets(&self) -> &[HotEntry] {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }

    fn request_cancel(&self) -> bool {
        if let Some(f) = self.request_cancel_fn {
            unsafe { f() };
            true
        } else {
            false
        }
    }

    fn reset_cancel(&self) -> bool {
        if let Some(f) = self.reset_cancel_fn {
            unsafe { f() };
            true
        } else {
            false
        }
    }
}

static GLOBAL_LIB: OnceLock<Arc<HotLibrary>> = OnceLock::new();

pub fn register_hot_library(lib: Arc<HotLibrary>) {
    let _ = GLOBAL_LIB.set(lib);
}

pub fn request_gpu_cancel() {
    if let Some(lib) = GLOBAL_LIB.get() {
        if lib.request_cancel() {
            return;
        }
    }
}

pub fn reset_gpu_cancel() {
    if let Some(lib) = GLOBAL_LIB.get() {
        if lib.reset_cancel() {
            return;
        }
    }
}
