use std::ffi::{CStr, CString};
use std::sync::atomic::Ordering;
use crate::engines::module_engine::module_engine::{ModuleEngine, PROGRESS_COMPLETED, PROGRESS_TOTAL};
use crate::engines::module_engine::sword_module::module::SwordModule;
use crate::engines::module_engine::sword_module::module_color::ModuleColor;
use crate::ffi::*;

impl ModuleEngine {
    // ------------------- REMOTE SOURCES -------------------

    pub fn get_remote_source_list(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut sources = Vec::new();
        unsafe {
            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(inner.install_mgr);
            org_crosswire_sword_InstallMgr_syncConfig(inner.install_mgr);

            let ptr = org_crosswire_sword_InstallMgr_getRemoteSources(inner.install_mgr);
            if !ptr.is_null() {
                let mut i = 0;
                while !(*ptr.offset(i)).is_null() {
                    sources.push(self.sword_ptr_to_string(*ptr.offset(i)).unwrap_or("Unknown".to_string()));
                    i += 1;
                }
            }
        }

        if sources.is_empty() {
            println!("[ModuleEngine] No remote sources found, using default sources");
            sources = vec![
                "CrossWire".to_string(),
                "IBT".to_string(),
                "ibiblio".to_string(),
            ];
        }

        println!("[ModuleEngine] Remote sources: {:?}", sources);
        sources
    }

    pub fn fetch_remote_modules(&self, source_name: &str) -> Vec<SwordModule> {
        let mut modules = Vec::new();
        let c_source = CString::new(source_name).unwrap();

        let path_str = self.sword_path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str).unwrap();

        unsafe {
            let local_install_mgr = org_crosswire_sword_InstallMgr_new(c_path.as_ptr(), None);
            let local_mgr = org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr());

            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(local_install_mgr);
            org_crosswire_sword_InstallMgr_refreshRemoteSource(
                local_install_mgr,
                c_source.as_ptr(),
            );

            org_crosswire_sword_InstallMgr_syncConfig(local_install_mgr);

            let info_ptr = org_crosswire_sword_InstallMgr_getRemoteModInfoList(
                local_install_mgr,
                local_mgr,
                c_source.as_ptr(),
            );

            if !info_ptr.is_null() {
                let mut i = 0;
                loop {
                    let entry = info_ptr.offset(i);
                    if entry.is_null() || (*entry).name.is_null() {
                        break;
                    }
                    let mut features_vec = Vec::new();
                    let feature_ptr_ptr = (*entry).features;

                    if !feature_ptr_ptr.is_null() {
                        let mut j = 0;
                        while !(*feature_ptr_ptr.offset(j)).is_null() {
                            let feature_c_str = CStr::from_ptr(*feature_ptr_ptr.offset(j));
                            features_vec.push(feature_c_str.to_string_lossy().into_owned());
                            j += 1;
                        }
                    }

                    let color_hash = format!(
                        "{}{}",
                        self.sword_ptr_to_string((*entry).name).unwrap_or("Unknown".to_string()),
                        self.sword_ptr_to_string((*entry).description).unwrap_or("Unknown".to_string())
                    );
                    modules.push(SwordModule {
                        name: self.sword_ptr_to_string((*entry).name).unwrap_or("Unknown".to_string()),
                        description: self.sword_ptr_to_string((*entry).description).unwrap_or("Unknown".to_string()),
                        category: self.sword_ptr_to_string((*entry).category).unwrap_or("Unknown".to_string()),
                        language: self.from_code(self.sword_ptr_to_string((*entry).language).unwrap_or("Unknown".to_string()).as_str()),
                        source: source_name.to_string(),
                        version: self.sword_ptr_to_string((*entry).version).unwrap_or("Unknown".to_string()),
                        delta: self.sword_ptr_to_string((*entry).delta).unwrap_or("Unknown".to_string()),
                        cipher_key: self.sword_ptr_to_string((*entry).cipherKey).unwrap_or("Unknown".to_string()),
                        features: features_vec,
                        signature_color: ModuleColor::generate(&color_hash),
                    });
                    i += 1;
                }
            }

            org_crosswire_sword_SWMgr_delete(local_mgr);
            org_crosswire_sword_InstallMgr_delete(local_install_mgr);
        }
        modules
    }

    // ------------------- INSTALL MODULE -------------------

    pub fn install_remote_module(&self, source: &str, module_name: &str) -> i32 {
        let c_source = CString::new(source).unwrap();
        let c_mod = CString::new(module_name).unwrap();

        PROGRESS_TOTAL.store(0, Ordering::SeqCst);
        PROGRESS_COMPLETED.store(0, Ordering::SeqCst);

        let path_str = self.sword_path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str).unwrap();

        unsafe {
            let local_install_mgr =
                org_crosswire_sword_InstallMgr_new(c_path.as_ptr(), Some(Self::status_reporter));
            let local_mgr = org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr());

            println!(
                "[ModuleEngine] Installing '{}' from '{}' (Background)",
                module_name, source
            );

            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(local_install_mgr);
            org_crosswire_sword_InstallMgr_refreshRemoteSource(
                local_install_mgr,
                c_source.as_ptr(),
            );

            org_crosswire_sword_InstallMgr_syncConfig(local_install_mgr);

            let res = org_crosswire_sword_InstallMgr_remoteInstallModule(
                local_install_mgr,
                local_mgr,
                c_source.as_ptr(),
                c_mod.as_ptr(),
            );
            println!("[ModuleEngine] Install result: {}", res);

            org_crosswire_sword_SWMgr_delete(local_mgr);
            org_crosswire_sword_InstallMgr_delete(local_install_mgr);

            if res == 0 {
                println!(
                    "[ModuleEngine] Installation successful, refreshing main engine awareness"
                );
                let mut inner = self.inner.lock().unwrap();
                self.rebuild_mgr(&mut inner);
            }

            res
        }
    }

    // ------------------- PROGRESS MONITOR -------------------

    pub fn get_download_progress(&self) -> f64 {
        let total = PROGRESS_TOTAL.load(Ordering::SeqCst);
        let completed = PROGRESS_COMPLETED.load(Ordering::SeqCst);

        if total == 0 {
            0.0
        } else {
            (completed as f64 / total as f64).clamp(0.0, 1.0)
        }
    }
}