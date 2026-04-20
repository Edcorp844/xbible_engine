use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // 1. --- BUILD THE SWORD ENGINE ---
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let config_file = root.join("cpp-bindings.toml");

    let config_text = fs::read_to_string(&config_file)
        .expect("Failed to read cpp-bindings.toml; please add it to the repository.");
    let config = config_text
        .parse::<toml::Value>()
        .expect("Failed to parse cpp-bindings.toml");

    let git_url = config
        .get("git_url")
        .and_then(|value| value.as_str())
        .expect("git_url is required in cpp-bindings.toml");

    let git_rev = config
        .get("git_rev")
        .and_then(|value| value.as_str())
        .unwrap_or("HEAD");

    let sword_src = root.join("sword");
    let sword_src = if sword_src.exists() {
        sword_src
    } else {
        let clone_dir = root.join("target").join("sword");
        if !clone_dir.exists() {
            let mut clone = Command::new("git");
            clone.arg("clone");
            if git_rev != "HEAD" {
                clone.arg("--depth").arg("1").arg("--branch").arg(git_rev);
            } else {
                clone.arg("--depth").arg("1");
            }
            clone.arg(git_url).arg(&clone_dir);
            let status = clone.status().expect("Failed to run git clone");
            if !status.success() {
                panic!("git clone failed with status {}", status);
            }
        }
        clone_dir
    };

    let dst = cmake::Config::new(&sword_src)
        .define("SWORD_BUILD_SHARED", "OFF")
        .define("SWORD_BUILD_EXAMPLES", "OFF")
        .define("SWORD_BUILD_TESTS", "OFF")
        .build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=sword");

    // 2. --- LINK SYSTEM DEPENDENCIES PER OS ---
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

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
        }
        _ => {
            let icu_uc = pkg_config::Config::new().probe("icu-uc").unwrap();
            let icu_i18n = pkg_config::Config::new().probe("icu-i18n").unwrap();

            for lib_path in icu_uc.link_paths.iter().chain(icu_i18n.link_paths.iter()) {
                println!("cargo:rustc-link-search=native={}", lib_path.display());
            }
            for lib in icu_uc.libs.iter().chain(icu_i18n.libs.iter()) {
                println!("cargo:rustc-link-lib=dylib={}", lib);
            }

            println!("cargo:rustc-link-lib=dylib=curl");
            println!("cargo:rustc-link-lib=dylib=z");
            println!("cargo:rustc-link-lib=dylib=bz2");
            println!("cargo:rustc-link-lib=dylib=lzma");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
    }

    // 3. --- GENERATE BINDINGS ---
    let include_path = dst.join("include");
    let header_path = include_path.join("sword").join("flatapi.h");

    let bindings = bindgen::Builder::default()
        .header(header_path.to_str().expect("Could not find flatapi.h"))
        .clang_arg(format!("-I{}", include_path.display()))
        .allowlist_function("org_crosswire_sword.*")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .rust_target(bindgen::RustTarget::Stable_1_77)
        .generate()
        .expect("Unable to generate bindings");

    let mut bindings_string = bindings.to_string();
    bindings_string = bindings_string.replace("extern \"C\" {", "unsafe extern \"C\" {");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_path.join("bindings.rs"), bindings_string)
        .expect("Couldn't write bindings!");
}
