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
    // Disable utilities completely to prevent binary layout configuration issues
    let utils_cmake = sword_src.join("utilities/CMakeLists.txt");
    if utils_cmake.exists() {
        let _ = fs::write(&utils_cmake, "# disabled by build.rs\n");
    }

    // Fix missing GLOBALREF/GLOBALDEF fallback layout macros in header file
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

        patched = patched.replace(
            "macro(add_subdirectory",
            "# macro(add_subdirectory removed by build.rs",
        );
        patched = patched.replace("endmacro()", "# endmacro() removed");

        patched = patched.replace(
            "find_package(CURL)",
            "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
        );
        patched = patched.replace(
            "FIND_PACKAGE(CURL)",
            "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
        );

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

    // Handle variable naming options between configurations for clone path localization
    let sword_src = manifest_dir.join("sword");
    let sword_src = if sword_src.exists() {
        sword_src
    } else {
        let clone_dir = out_dir.join("sword_source_isolated");
        if !clone_dir.exists() {
            let branch_args: &[&str] = if git_rev != "HEAD" {
                &["--branch", git_rev]
            } else {
                &[]
            };
            git_clone(git_url, &clone_dir, branch_args);
        }
        clone_dir
    };

    // iOS SDK detection flags
    let mut sdk_inc_path: Option<String> = None;
    let mut sdk_name_flag: Option<String> = None;

    if target_os == "ios" {
        let is_sim = target_triple.contains("sim") || target_triple.contains("ios-sim");
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

    // CMake Setup Configuration Engine
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

    // Standard platform definitions passing
    if target_os == "ios" || target_os == "macos" {
        cmake.cflag("-D__unix__").cxxflag("-D__unix__");
    }

    // Pull Host/Target Architecture strings matching raw structural layout rules
    let raw_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "arm64".to_string());
    let arch = if raw_arch == "aarch64" {
        "arm64"
    } else {
        &raw_arch
    };

    match target_os.as_str() {
        "ios" => {
            let sdk_name = sdk_name_flag.unwrap();
            let sdk_inc = sdk_inc_path.unwrap();
            let is_simulator = target_triple.contains("sim") || target_triple.contains("ios-sim");

            // Explicit target alignment arguments passed directly to rustc compilation cycle
            if is_simulator {
                println!("cargo:rustc-link-arg=-target");
                println!("cargo:rustc-link-arg=arm64-apple-ios14.0.0-simulator");
            } else {
                println!("cargo:rustc-link-arg=-target");
                println!("cargo:rustc-link-arg=arm64-apple-ios14.0.0");
            }

            cmake
                .define("CMAKE_OSX_SYSROOT", sdk_name)
                .define("CMAKE_SYSTEM_NAME", "iOS")
                .define("CMAKE_OSX_ARCHITECTURES", arch)
                .define("CMAKE_OSX_DEPLOYMENT_TARGET", "14.0")
                .define("CURL_FOUND", "TRUE")
                .define("CURL_INCLUDE_DIR", &sdk_inc)
                .define("CURL_INCLUDE_DIRS", &sdk_inc)
                .define("CURL_LIBRARIES", "")
                .define("CMAKE_INCLUDE_PATH", &sdk_inc);
        }
        "macos" => {
            cmake.define("CMAKE_OSX_ARCHITECTURES", arch);
            cmake.define("CMAKE_OSX_DEPLOYMENT_TARGET", "14.0");
            println!("cargo:rustc-link-arg=-mmacosx-version-min=14.0");
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
                    .define("SWORD_CURL", "OFF")
                    .define("ANDROID_ABI", abi)
                    .define("ANDROID_PLATFORM", "android-24")
                    .define("CMAKE_SHARED_LINKER_FLAGS", "-llog")
                    .cflag("-DIOAPI_NO_64")
                    .cxxflag("-DIOAPI_NO_64");
            }
        }
        _ => {}
    }

    let dst = cmake.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=sword");

    // ─────────────────────────────────────────────────────────────────────────────
    //  Platform Specific Linker Flag Bindings
    // ─────────────────────────────────────────────────────────────────────────────
    match target_os.as_str() {
        "windows" => {
            println!("cargo:rustc-link-lib=static=z");
            println!("cargo:rustc-link-lib=static=bz2");
            println!("cargo:rustc-link-lib=static=lzma");
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=ws2_32");
            println!("cargo:rustc-link-lib=dylib=crypt32");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=lzma");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        }
        "ios" => {
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=lzma");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CFNetwork");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        }
        "android" => {
            //println!("cargo:rustc-link-lib=dylib=curl");
            cmake.define("SWORD_CURL", "OFF");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=c++_shared");
            println!("cargo:rustc-link-lib=dylib=log");
        }
        _ => {
            if let Ok(icu_uc) = pkg_config::Config::new().probe("icu-uc") {
                if let Ok(icu_i18n) = pkg_config::Config::new().probe("icu-i18n") {
                    for lib_path in icu_uc.link_paths.iter().chain(icu_i18n.link_paths.iter()) {
                        println!("cargo:rustc-link-search=native={}", lib_path.display());
                    }
                    for lib in icu_uc.libs.iter().chain(icu_i18n.libs.iter()) {
                        println!("cargo:rustc-link-lib=dylib={}", lib);
                    }
                }
            }
            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=lzma");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────────
    //  Bindgen Configuration
    // ─────────────────────────────────────────────────────────────────────────────
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

    if target_os == "android" {
        if let Ok(ndk) = env::var("ANDROID_NDK_HOME") {
            let sysroot = Path::new(&ndk).join("toolchains/llvm/prebuilt/darwin-x86_64/sysroot");
            let clang_target = if target_triple.contains("x86_64") {
                "x86_64-linux-android24"
            } else if target_triple.contains("aarch64") {
                "aarch64-linux-android24"
            } else {
                "armv7a-linux-androideabi24"
            };
            builder = builder
                .clang_arg(format!("--target={}", clang_target))
                .clang_arg(format!("--sysroot={}", sysroot.display()));
        }
    }

    builder
        .generate()
        .expect("bindgen failed")
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings.rs");
}
