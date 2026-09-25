use std::env;
use std::fs;
use std::io::Read;
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

fn list_dir(p: &Path) -> String {
    match fs::read_dir(p) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n  "),
        Err(_) => "<unreadable>".to_string(),
    }
}

fn is_valid_static_archive(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];
    if f.read_exact(&mut magic).is_err() {
        return false;
    }
    &magic == b"!<arch>\n"
}

/// Ensure `src` becomes a thin Unix static archive named `lib{name}.a` in `out_dir`.
/// Fat Mach-O universal binaries are thinned with `lipo -thin <arch>`.
fn thin_static_lib(src: &Path, out_dir: &Path, arch: &str, name: &str) -> PathBuf {
    let dest = out_dir.join(format!("lib{name}.a"));

    if is_valid_static_archive(src) {
        if src != dest {
            fs::copy(src, &dest).unwrap_or_else(|e| {
                panic!("copy {} -> {}: {}", src.display(), dest.display(), e)
            });
        }
        return dest;
    }

    // Fat binary (cafe babe) or other format → thin with lipo
    println!(
        "cargo:warning=Thinning {} to arch {} -> {}",
        src.display(),
        arch,
        dest.display()
    );

    let status = Command::new("lipo")
        .args([
            "-thin",
            arch,
            src.to_str().unwrap(),
            "-output",
            dest.to_str().unwrap(),
        ])
        .status()
        .unwrap_or_else(|e| panic!("failed to run lipo: {e}"));

    assert!(
        status.success(),
        "lipo -thin {} failed for {}",
        arch,
        src.display()
    );
    assert!(
        is_valid_static_archive(&dest),
        "after lipo, {} is still not a valid ar archive (expected !<arch>)",
        dest.display()
    );
    dest
}

// ─────────────────────────────────────────────────────────────────────────────
//  Pre-built libcurl / OpenSSL / nghttp2 for iOS only
//  https://github.com/jasonacox/Build-OpenSSL-cURL/releases/tag/1.0.3
// ─────────────────────────────────────────────────────────────────────────────
struct CurlArtifacts {
    include_dir: PathBuf,
    lib_dir: PathBuf, // directory containing thinned libcurl.a, libssl.a, ...
    has_ssl: bool,
    has_crypto: bool,
    has_nghttp2: bool,
}

fn locate_prebuilt_curl(
    manifest_dir: &Path,
    out_dir: &Path,
    is_simulator: bool,
    arch: &str,
) -> CurlArtifacts {
    let thin_arch = if arch == "aarch64" || arch == "arm64" {
        "arm64"
    } else {
        "x86_64"
    };

    // ── (1) Optional vendor override ───────────────────────────────────────
    let vendor = manifest_dir.join("vendor/curl-ios");
    if vendor.join("include/curl/curl.h").exists() {
        let sub = if is_simulator { "ios-sim" } else { "ios" };
        let mut lib_dir = vendor.join("lib").join(sub);
        if !lib_dir.join("libcurl.a").exists() {
            lib_dir = vendor.join("lib");
        }
        if lib_dir.join("libcurl.a").exists() {
            let thin_out = out_dir.join("thinned_curl_libs");
            fs::create_dir_all(&thin_out).expect("mkdir thinned_curl_libs");

            thin_static_lib(&lib_dir.join("libcurl.a"), &thin_out, thin_arch, "curl");
            let has_ssl = lib_dir.join("libssl.a").exists();
            let has_crypto = lib_dir.join("libcrypto.a").exists();
            let has_nghttp2 = lib_dir.join("libnghttp2.a").exists();
            if has_ssl {
                thin_static_lib(&lib_dir.join("libssl.a"), &thin_out, thin_arch, "ssl");
            }
            if has_crypto {
                thin_static_lib(&lib_dir.join("libcrypto.a"), &thin_out, thin_arch, "crypto");
            }
            if has_nghttp2 {
                thin_static_lib(
                    &lib_dir.join("libnghttp2.a"),
                    &thin_out,
                    thin_arch,
                    "nghttp2",
                );
            }

            println!(
                "cargo:warning=Using vendored curl-ios (thinned {}) from {}",
                thin_arch,
                thin_out.display()
            );
            return CurlArtifacts {
                include_dir: vendor.join("include"),
                lib_dir: thin_out,
                has_ssl,
                has_crypto,
                has_nghttp2,
            };
        }
    }

    // ── (2) Download 1.0.3 archive into OUT_DIR ─────────────────────────────
    let build_root = out_dir.join("downloaded_curl_ios");
    let archive_dir = build_root.join("archive");
    let expected_root_name = "libcurl-8.17.0-openssl-3.0.18-nghttp2-1.68.0";
    let extracted_root = archive_dir.join(expected_root_name);

    let base_path = if extracted_root.exists() {
        extracted_root.clone()
    } else {
        archive_dir.clone()
    };

    let curl_inc = base_path.join("include");
    let already_ok = curl_inc.join("curl/curl.h").exists()
        && (base_path.join("lib").exists() || base_path.join("xcframework").exists());

    if !already_ok {
        fs::create_dir_all(&build_root).expect("mkdir build_root");
        if archive_dir.exists() {
            let _ = fs::remove_dir_all(&archive_dir);
        }
        fs::create_dir_all(&archive_dir).expect("mkdir archive_dir");

        let download_url = "https://github.com/jasonacox/Build-OpenSSL-cURL/releases/download/1.0.3/libcurl-8.17.0-openssl-3.0.18-nghttp2-1.68.0.tgz";
        let tar_path = build_root.join("curl_prebuilt.tgz");

        println!(
            "cargo:warning=Downloading iOS libcurl 1.0.3 from {} ...",
            download_url
        );

        let mut curl_cmd = Command::new("curl");
        curl_cmd.args(["-L", "-f", "-o", tar_path.to_str().unwrap(), download_url]);
        run(&mut curl_cmd, "Download pre-built libcurl tarball");

        let mut tar_cmd = Command::new("tar");
        tar_cmd.args([
            "-xzf",
            tar_path.to_str().unwrap(),
            "-C",
            archive_dir.to_str().unwrap(),
        ]);
        run(&mut tar_cmd, "Extract pre-built libcurl archive");
    }

    let final_base = if archive_dir.join(expected_root_name).exists() {
        archive_dir.join(expected_root_name)
    } else {
        archive_dir.clone()
    };

    let include_dir = final_base.join("include");
    if !include_dir.join("curl/curl.h").exists() {
        panic!(
            "curl headers not found under {}. Contents of final_base:\n  {}",
            include_dir.display(),
            list_dir(&final_base)
        );
    }

    // ── Resolve source .a paths (prefer XCFramework) ───────────────────────
    let xc_root = final_base.join("xcframework");
    let slice_name = if is_simulator {
        "ios-arm64_x86_64-simulator"
    } else {
        "ios-arm64_arm64e"
    };

    let mut curl_src = xc_root
        .join("libcurl.xcframework")
        .join(slice_name)
        .join("libcurl.a");
    let mut ssl_src = xc_root
        .join("libssl.xcframework")
        .join(slice_name)
        .join("libssl.a");
    let mut crypto_src = xc_root
        .join("libcrypto.xcframework")
        .join(slice_name)
        .join("libcrypto.a");
    let mut nghttp2_src = xc_root
        .join("libnghttp2.xcframework")
        .join(slice_name)
        .join("libnghttp2.a");

    if !curl_src.exists() {
        // Fallback: lib/iOS or lib/iOS-simulator (fat binaries — will be thinned)
        let platform = if is_simulator {
            final_base.join("lib").join("iOS-simulator")
        } else {
            final_base.join("lib").join("iOS")
        };
        curl_src = platform.join("libcurl.a");
        ssl_src = platform.join("libssl.a");
        crypto_src = platform.join("libcrypto.a");
        nghttp2_src = platform.join("libnghttp2.a");
        println!(
            "cargo:warning=XCFramework slice '{}' missing; falling back to {}",
            slice_name,
            platform.display()
        );
    } else {
        println!(
            "cargo:warning=Using XCFramework slice '{}' (will thin to {})",
            slice_name, thin_arch
        );
    }

    if !curl_src.exists() {
        panic!(
            "libcurl.a not found.\n\
             Tried XCFramework slice '{}' and lib/iOS*.\n\
             xcframework/libcurl.xcframework contents:\n  {}\n\
             lib contents:\n  {}",
            slice_name,
            list_dir(&xc_root.join("libcurl.xcframework")),
            list_dir(&final_base.join("lib"))
        );
    }

    // ── Thin everything into one directory ─────────────────────────────────
    let thin_out = out_dir.join("thinned_curl_libs");
    fs::create_dir_all(&thin_out).expect("mkdir thinned_curl_libs");

    thin_static_lib(&curl_src, &thin_out, thin_arch, "curl");

    let has_ssl = ssl_src.exists();
    let has_crypto = crypto_src.exists();
    let has_nghttp2 = nghttp2_src.exists();

    if has_ssl {
        thin_static_lib(&ssl_src, &thin_out, thin_arch, "ssl");
    }
    if has_crypto {
        thin_static_lib(&crypto_src, &thin_out, thin_arch, "crypto");
    }
    if has_nghttp2 {
        thin_static_lib(&nghttp2_src, &thin_out, thin_arch, "nghttp2");
    }

    println!(
        "cargo:warning=Thinned iOS curl libs ({}) ready at {}",
        thin_arch,
        thin_out.display()
    );

    CurlArtifacts {
        include_dir,
        lib_dir: thin_out,
        has_ssl,
        has_crypto,
        has_nghttp2,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Patch SWORD CMakeLists
// ─────────────────────────────────────────────────────────────────────────────
fn patch_sword_cmakelists(
    sword_src: &Path,
    target_os: &str,
    sdk_include_path: Option<&str>,
    enable_curl: bool,
) {
    let utils_cmake = sword_src.join("utilities/CMakeLists.txt");
    if utils_cmake.exists() {
        let _ = fs::write(&utils_cmake, "# disabled by build.rs\n");
    }

    let ftplib = sword_src.join("include/ftplib.h");
    if ftplib.exists() {
        if let Ok(mut src) = fs::read_to_string(&ftplib) {
            if !src.contains("#ifndef GLOBALREF") {
                src.insert_str(
                    0,
                    "#ifndef GLOBALREF\n#define GLOBALREF extern\n#endif\n#ifndef GLOBALDEF\n#define GLOBALDEF\n#endif\n\n",
                );
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

        if enable_curl {
            patched = patched.replace(
                "find_package(CURL)",
                "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
            );
            patched = patched.replace(
                "FIND_PACKAGE(CURL)",
                "# bypassed by build.rs\nset(CURL_FOUND TRUE)",
            );
        } else {
            patched = patched.replace(
                "find_package(CURL)",
                "# curl disabled by build.rs\nset(CURL_FOUND FALSE)",
            );
            patched = patched.replace(
                "FIND_PACKAGE(CURL)",
                "# curl disabled by build.rs\nset(CURL_FOUND FALSE)",
            );
        }

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

        let curl_on = if enable_curl { "ON" } else { "OFF" };
        let curl_found = if enable_curl { "TRUE" } else { "FALSE" };

        let header = format!(
            r#"cmake_minimum_required(VERSION 3.10)
set(SWORD_BUILD_SHARED OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(SWORD_BUILD_UTILS OFF CACHE BOOL "" FORCE)
set(SWORD_CURL {curl_on} CACHE BOOL "" FORCE)
set(CURL_FOUND {curl_found} CACHE BOOL "" FORCE)
set(NOTESTS TRUE)
"#
        );

        patched = header + &patched;
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

    let cfg_text = fs::read_to_string(manifest_dir.join("cpp-bindings.toml"))
        .expect("cpp-bindings.toml not found");
    let cfg: toml::Value = cfg_text.parse().expect("Failed to parse cpp-bindings.toml");

    let git_url = cfg["git_url"].as_str().unwrap();
    let git_rev = cfg
        .get("git_rev")
        .and_then(|v| v.as_str())
        .unwrap_or("HEAD");

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

    // CURL feature on for these OSes; prebuilt static libs only on iOS
    let enable_curl = matches!(
        target_os.as_str(),
        "macos" | "windows" | "linux" | "ios"
    );

    let raw_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "arm64".to_string());
    let arch = if raw_arch == "aarch64" {
        "arm64"
    } else {
        raw_arch.as_str()
    };
    let is_simulator = target_triple.contains("sim")
        || target_triple.contains("ios-sim")
        || (target_os == "ios" && arch == "x86_64");

    let mut sdk_inc_path: Option<String> = None;
    let mut sdk_name_flag: Option<String> = None;
    let mut ios_sdk_path: Option<String> = None;
    let mut apple_target_flag: Option<String> = None;

    if target_os == "ios" {
        let sdk_name = if is_simulator {
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
        ios_sdk_path = Some(sdk_path_str.clone());

        let sdk_usr = Path::new(&sdk_path_str).join("usr");
        sdk_inc_path = Some(sdk_usr.join("include").to_string_lossy().into_owned());

        apple_target_flag = Some(if is_simulator {
            format!("{}-apple-ios14.0-simulator", arch)
        } else {
            format!("{}-apple-ios14.0", arch)
        });
    }

    patch_sword_cmakelists(
        &sword_src,
        &target_os,
        sdk_inc_path.as_deref(),
        enable_curl,
    );

    let mut cmake = cmake::Config::new(&sword_src);
    cmake
        .define("SWORD_BUILD_SHARED", "OFF")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("SWORD_BUILD_EXAMPLES", "OFF")
        .define("SWORD_BUILD_TESTS", "OFF")
        .define("SWORD_BUILD_UTILS", "OFF")
        .define("SWORD_CURL", if enable_curl { "ON" } else { "OFF" });

    if let Some(inc) = &sdk_inc_path {
        let flags = format!("-I{} -fPIC", inc);
        cmake
            .env("CFLAGS", &flags)
            .env("CXXFLAGS", &flags)
            .cflag(&flags)
            .cxxflag(&flags);
    }

    if target_os == "ios" || target_os == "macos" {
        cmake.cflag("-D__unix__").cxxflag("-D__unix__");
    }

    let mut curl_link_dir: Option<PathBuf> = None;
    let mut extra_link_libs: Vec<&'static str> = Vec::new();

    match target_os.as_str() {
        "ios" => {
            let sdk_name = sdk_name_flag.as_ref().unwrap();
            let sdk_inc = sdk_inc_path.as_ref().unwrap();
            let apple_target = apple_target_flag.as_ref().unwrap();

            let cc = Command::new("xcrun")
                .args(["--sdk", sdk_name, "--find", "clang"])
                .output()
                .expect("xcrun clang failed");
            let cxx = Command::new("xcrun")
                .args(["--sdk", sdk_name, "--find", "clang++"])
                .output()
                .expect("xcrun clang++ failed");
            let cc = String::from_utf8_lossy(&cc.stdout).trim().to_string();
            let cxx = String::from_utf8_lossy(&cxx.stdout).trim().to_string();

            let art = locate_prebuilt_curl(&manifest_dir, &out_dir, is_simulator, arch);

            println!("cargo:rustc-link-arg=-target");
            println!("cargo:rustc-link-arg={}", apple_target);
            println!("cargo:rustc-link-search=native={}", art.lib_dir.display());

            curl_link_dir = Some(art.lib_dir.clone());

            if art.has_ssl {
                extra_link_libs.push("ssl");
            }
            if art.has_crypto {
                extra_link_libs.push("crypto");
            }
            if art.has_nghttp2 {
                extra_link_libs.push("nghttp2");
            }

            let curl_cflags = format!("-I{}", art.include_dir.display());
            let curl_lib = art.lib_dir.join("libcurl.a");

            cmake
                .define("CMAKE_C_COMPILER", &cc)
                .define("CMAKE_CXX_COMPILER", &cxx)
                .define("CMAKE_ASM_COMPILER", &cc)
                .define("CMAKE_OSX_SYSROOT", sdk_name.as_str())
                .define("CMAKE_SYSTEM_NAME", "iOS")
                .define("CMAKE_OSX_ARCHITECTURES", arch)
                .define("CMAKE_OSX_DEPLOYMENT_TARGET", "14.0")
                .define("CMAKE_INCLUDE_PATH", sdk_inc.as_str())
                .define("SWORD_CURL", "ON")
                .define("CURL_FOUND", "TRUE")
                .define("CURL_INCLUDE_DIR", art.include_dir.to_str().unwrap())
                .define("CURL_LIBRARY", curl_lib.to_str().unwrap())
                .cflag(&curl_cflags)
                .cxxflag(&curl_cflags);
        }
        "macos" => {
            cmake.define("CMAKE_OSX_ARCHITECTURES", arch);
            cmake.define("CMAKE_OSX_DEPLOYMENT_TARGET", "14.0");
            cmake.define("SWORD_CURL", "ON");
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
                    .define("CURL_FOUND", "FALSE")
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
            println!("cargo:rustc-link-lib=static=curl");
            for lib in &extra_link_libs {
                println!("cargo:rustc-link-lib=static={}", lib);
            }
             println!("cargo:rustc-link-lib=static=bz2");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=c++");
            println!("cargo:rustc-link-lib=framework=CFNetwork");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=framework=SystemConfiguration");
        }
        "android" => {
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

    let include_dir = dst.join("include");
    let header = include_dir.join("sword/flatapi.h");

    let mut builder = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("org_crosswire_sword.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_target(bindgen::RustTarget::stable(96, 0).unwrap())
        .wrap_unsafe_ops(true);

    match target_os.as_str() {
        "ios" => {
            let sdk_path = ios_sdk_path.expect("iOS SDK path");
            let clang_target = apple_target_flag.expect("apple target");

            builder = builder
                .clang_arg(format!("--target={}", clang_target))
                .clang_arg(format!("-isysroot{}", sdk_path))
                .clang_arg(format!("-I{}/usr/include", sdk_path));

            if is_simulator {
                builder = builder.clang_arg("-mios-simulator-version-min=14.0");
            } else {
                builder = builder.clang_arg("-miphoneos-version-min=14.0");
            }

            if let Some(dir) = curl_link_dir.as_ref() {
                // Headers live next to the downloaded archive, not the thinned dir
                // (include path already passed via CURL_INCLUDE_DIR / cmake)
                let _ = dir;
            }
        }
        "macos" => {
            if let Ok(sdk) = env::var("SDKROOT") {
                builder = builder.clang_arg(format!("--sysroot={}", sdk));
            } else {
                let sdk_output = Command::new("xcrun")
                    .args(["--sdk", "macosx", "--show-sdk-path"])
                    .output()
                    .expect("xcrun macosx failed");
                let sdk_path = String::from_utf8_lossy(&sdk_output.stdout)
                    .trim()
                    .to_string();
                builder = builder.clang_arg(format!("-isysroot{}", sdk_path));
            }
        }
        "android" => {
            if let Ok(ndk) = env::var("ANDROID_NDK_HOME") {
                let sysroot =
                    Path::new(&ndk).join("toolchains/llvm/prebuilt/darwin-x86_64/sysroot");
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
        _ => {}
    }

    builder
        .generate()
        .expect("bindgen failed")
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Failed to write bindings.rs");
}