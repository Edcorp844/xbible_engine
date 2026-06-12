use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &mut Command, description: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to spawn `{}`: {}", description, e));
    assert!(
        status.success(),
        "`{}` exited with status {}",
        description,
        status
    );
}

fn git_clone(url: &str, dest: &Path, extra_args: &[&str]) {
    if dest.exists() {
        return;
    }
    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--depth").arg("1");
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg(url).arg(dest);
    run(&mut cmd, &format!("git clone {}", url));
}

// ─────────────────────────────────────────────────────────────────────────────
//  Patch SWORD CMakeLists
// ─────────────────────────────────────────────────────────────────────────────

fn patch_sword_cmakelists(sword_src: &Path, target_os: &str, sdk_include_path: Option<&str>) {
    // Disable utilities
    let _ = fs::write(
        sword_src.join("utilities/CMakeLists.txt"),
        "# disabled by build.rs\n",
    );

    // Fix ftplib.h
    let ftplib = sword_src.join("include/ftplib.h");
    if ftplib.exists() {
        if let Ok(mut src) = fs::read_to_string(&ftplib) {
            if !src.contains("#ifndef GLOBALREF") {
                src.insert_str(0, "#ifndef GLOBALREF\n#define GLOBALREF extern\n#endif\n#ifndef GLOBALDEF\n#define GLOBALDEF\n#endif\n\n");
                let _ = fs::write(&ftplib, src);
            }
        }
    }

    let cmake_path = sword_src.join("CMakeLists.txt");
    if let Ok(content) = fs::read_to_string(&cmake_path) {
        let mut patched = content;

        // Remove any previous macro definitions that may cause recursion
        patched = patched.replace(
            "macro(add_subdirectory",
            "# macro(add_subdirectory removed by build.rs",
        );
        patched = patched.replace("endmacro()", "# endmacro() removed");

        // Bypass FindCURL
        patched = patched.replace(
            "find_package(CURL)",
            "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
        );
        patched = patched.replace(
            "FIND_PACKAGE(CURL)",
            "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
        );

        // Force CURL ON
        if !patched.contains("SWORD_CURL") {
            patched = patched.replace(
                "project(",
                "option(SWORD_CURL \"Enable network support\" ON)\n\nproject(",
            );
        }

        if target_os == "ios" {
            patched = patched.replace("SHARED", "STATIC");
            patched = patched.replace("add_library(sword ", "add_library(sword STATIC ");
            patched = patched.replace("add_library( sword ", "add_library(sword STATIC ");

            patched = patched.replace("sword sword_static", "sword_static");
            patched = patched.replace(
                "TARGETS sword DESTINATION",
                "TARGETS sword_static DESTINATION",
            );

            if let Some(inc) = sdk_include_path {
                let injection = format!("include_directories(\"{}\")\n", inc);
                if !patched.contains(&injection) {
                    patched = injection + &patched;
                }
            }
        }

        // Safe overrides at the top
        let header = r#"cmake_minimum_required(VERSION 3.10)

set(SWORD_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_UTILS OFF CACHE BOOL "" FORCE)
set(SWORD_CURL ON CACHE BOOL "" FORCE)
set(CURL_FOUND TRUE CACHE BOOL "" FORCE)
set(NOTESTS TRUE)
"#;

        patched = header.to_string() + &patched;
        let _ = fs::write(&cmake_path, patched);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Main
// ─────────────────────────────────────────────────────────────────────────────

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_triple = env::var("TARGET").unwrap();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Read config
    let cfg_text = fs::read_to_string(manifest_dir.join("cpp-bindings.toml"))
        .expect("cpp-bindings.toml not found");
    let cfg: toml::Value = cfg_text.parse().expect("Failed to parse cpp-bindings.toml");

    let git_url = cfg["git_url"].as_str().unwrap();
    let git_rev = cfg
        .get("git_rev")
        .and_then(|v| v.as_str())
        .unwrap_or("HEAD");

    // Clone SWORD
    let sword_src = out_dir.join("sword_source_isolated");
    if !sword_src.exists() {
        let branch_args: &[&str] = if git_rev != "HEAD" {
            &["--branch", git_rev]
        } else {
            &[]
        };
        git_clone(git_url, &sword_src, branch_args);
    }

    // iOS SDK detection
    let mut sdk_inc_path: Option<String> = None;
    let mut sdk_name_flag: Option<String> = None;

    if target_os == "ios" {
        let is_sim = target_triple.contains("sim");
        let sdk_name = if is_sim {
            "iphonesimulator"
        } else {
            "iphoneos"
        };
        sdk_name_flag = Some(sdk_name.to_string());

        let sdk_output = Command::new("xcrun")
            .args(["--sdk", sdk_name, "--show-sdk-path"])
            .output()
            .expect("xcrun failed");

        let sdk_path_str = String::from_utf8_lossy(&sdk_output.stdout)
            .trim()
            .to_string();
        let sdk_usr = Path::new(&sdk_path_str).join("usr");
        sdk_inc_path = Some(sdk_usr.join("include").to_string_lossy().into_owned());
    }

    patch_sword_cmakelists(&sword_src, &target_os, sdk_inc_path.as_deref());

    // CMake setup
    let mut cmake = cmake::Config::new(&sword_src);

    cmake
        .define("SWORD_BUILD_SHARED", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("SWORD_BUILD_EXAMPLES", "OFF")
        .define("SWORD_BUILD_TESTS", "OFF")
        .define("SWORD_BUILD_UTILS", "OFF")
        .define("SWORD_CURL", "ON");

    if let Some(inc) = &sdk_inc_path {
        let flags = format!("-I{} -fPIC", inc);
        cmake
            .env("CFLAGS", &flags)
            .env("CXXFLAGS", &flags)
            .cflag(&flags)
            .cxxflag(&flags);
    }

    match target_os.as_str() {
        "ios" => {
            let sdk_name = sdk_name_flag.unwrap();
            let sdk_inc = sdk_inc_path.unwrap();

            // 1. Detect if we are building for device or simulator
            let target_triple = std::env::var("TARGET").unwrap_or_default();
            let is_simulator = target_triple.contains("ios-sim");

            // 2. Select and append the exact matching deployment target payload
            println!("cargo:rustc-link-arg=-target");
            if is_simulator {
                println!("cargo:rustc-link-arg=arm64-apple-ios14.0.0-simulator");
            } else {
                println!("cargo:rustc-link-arg=arm64-apple-ios14.0.0");
            }

            // Pull the arch string and correct 'aarch64' to Apple's 'arm64'
            let raw_arch =
                std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "arm64".to_string());
            let arch = if raw_arch == "aarch64" {
                "arm64"
            } else {
                &raw_arch
            };

            cmake
                .define("CMAKE_OSX_SYSROOT", sdk_name)
                .define("CMAKE_SYSTEM_NAME", "iOS")
                .define("CMAKE_OSX_ARCHITECTURES", arch)
                .define("CURL_FOUND", "TRUE")
                .define("CURL_INCLUDE_DIR", &sdk_inc)
                .define("CURL_INCLUDE_DIRS", &sdk_inc)
                // CHANGE THIS LINE: Change "-lcurl" to ""
                .define("CURL_LIBRARIES", "")
                .define("CMAKE_INCLUDE_PATH", &sdk_inc)
                .cflag("-D__unix__")
                .cxxflag("-D__unix__");

            // Framework bindings
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CFNetwork");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        }
        "macos" => {
            cmake.cflag("-D__unix__").cxxflag("-D__unix__");
        }
        "android" => {
            if let Ok(ndk) = env::var("ANDROID_NDK_HOME") {
                let tc = Path::new(&ndk).join("build/cmake/android.toolchain.cmake");
                if tc.exists() {
                    cmake.define("CMAKE_TOOLCHAIN_FILE", tc.to_str().unwrap());
                }
                let abi = if target_triple.contains("x86_64") {
                    "x86_64"
                } else {
                    "arm64-v8a"
                };
                cmake
                    .define("ANDROID_ABI", abi)
                    .define("ANDROID_PLATFORM", "android-21");
            }
        }
        _ => {}
    }

    let dst = cmake.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=sword");

    // Platform-specific linking
    match target_os.as_str() {
        "ios" => {
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CFNetwork");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        }
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
        }
        "android" => {
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=c++_shared");
            println!("cargo:rustc-link-lib=dylib=log");
        }
        _ => {
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }

    // Bindgen
    let include_dir = dst.join("include");
    let header = include_dir.join("sword/flatapi.h");

    let mut builder = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("org_crosswire_sword.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_target(bindgen::RustTarget::stable(96, 0).unwrap())
        .wrap_unsafe_ops(true);

    if target_os == "ios" || target_os == "macos" {
        if let Ok(sdk) = env::var("SDKROOT") {
            builder = builder.clang_arg(format!("--sysroot={}", sdk));
        }
    }

    builder
        .generate()
        .expect("bindgen failed")
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings.rs");
}
