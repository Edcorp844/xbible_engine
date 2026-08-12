use crate::{
    engines::module_engine::module_engine::ModuleEngine,
    ffi::org_crosswire_sword_SWMgr_setGlobalOption,
};
use std::ffi::{c_char, CStr, CString};

impl ModuleEngine {
    /// Safely converts any raw C-string pointer (*const i8, *const u8, *mut u8, etc.)
    /// into an Option<String>, handling ARM/x86 c_char signedness differences automatically.
    pub(crate) unsafe fn sword_ptr_to_string<T>(&self, ptr: *const T) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let c_str = unsafe { CStr::from_ptr(ptr.cast::<c_char>()) };
        Some(c_str.to_string_lossy().into_owned())
    }

    pub(crate) unsafe fn set_global_options(&self, options: &[&str], state: &str) {
        let state_c = CString::new(state).unwrap();
        for opt in options {
            let opt_c = CString::new(*opt).unwrap();
            unsafe {
                org_crosswire_sword_SWMgr_setGlobalOption(
                    self.inner.lock().unwrap().mgr,
                    opt_c.as_ptr(),
                    state_c.as_ptr(),
                );
            }
        }
    }
}