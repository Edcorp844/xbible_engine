use crate::{ffi::org_crosswire_sword_SWMgr_setGlobalOption, sword_engine::module_engine::sword_engine::SwordEngine};


impl SwordEngine {
    pub(crate) unsafe fn sword_ptr_to_string(
        &self,
        ptr: *const std::os::raw::c_char,
    ) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned() })
    }

    pub(crate) unsafe fn set_global_options(&self, options: &[&str], state: &str) {
        let state_c = std::ffi::CString::new(state).unwrap();
        for opt in options {
            let opt_c = std::ffi::CString::new(*opt).unwrap();
            unsafe {
                org_crosswire_sword_SWMgr_setGlobalOption(
                    self.inner.lock().unwrap().mgr,
                    opt_c.as_ptr(),
                    state_c.as_ptr(),
                )
            };
        }
    }
}
