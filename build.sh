#!/bin/bash

export MACOSX_DEPLOYMENT_TARGET=14.0

# --- Styling ---
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
MAGENTA='\033[0;35m'
BLUE='\033[0;34m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

# --- Configuration ---
LIB_NAME="xbible_engine" 
SWIFT_PKG_DIR="../Bible_engine_swift" 

# --- Target Data ---
# Structure: Label | Triple | Extension | OS Folder
TARGETS=(
    "macOS (Intel)"        "x86_64-apple-darwin"      "a"   "macOS"
    "macOS (Silicon)"      "aarch64-apple-darwin"    "a"   "macOS"
    "iOS (Sim)"            "aarch64-apple-ios-sim"   "a"   "iOS"
    "iOS (Device)"         "aarch64-apple-ios"       "a"   "iOS"
    "Android (ARM64)"      "aarch64-linux-android"   "so"  "Android"
    "Android (x86_64/Sim)" "x86_64-linux-android"    "so"  "Android"
    "Linux (x86_64)"       "x86_64-unknown-linux-gnu" "so" "Linux"
    "Windows (x86_64)"     "x86_64-pc-windows-msvc"  "dll" "Windows"
)

LANGS=("swift" "kotlin" "csharp" "java" "c" "cpp" "python" "ruby")

echo -e "${MAGENTA}${BOLD}=======================================${NC}"
echo -e "${MAGENTA}${BOLD}    xbible_engine: Universal Build     ${NC}"
echo -e "${MAGENTA}${BOLD}=======================================${NC}"

# Step 1: Select Platform(s)
echo -e "${YELLOW}1. Select Target Platforms (Recommended: 2 3 4):${NC}"
for ((i=0; i<${#TARGETS[@]}/4; i++)); do
    echo -e "${CYAN}$((i+1)))${NC} ${TARGETS[i*4]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r plat_choices

# Step 2: Select Language(s)
echo -e "\n${YELLOW}2. Select Binding Languages:${NC}"
for i in "${!LANGS[@]}"; do
    echo -e "${CYAN}$((i+1)))${NC} ${LANGS[$i]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r lang_choices

# Trackers for XCFramework
SWIFT_SELECTED=false
MACOS_BUILT=false
IOS_SIM_BUILT=false
IOS_DEV_BUILT=false

for p_choice in $plat_choices; do
    idx=$(( (p_choice - 1) * 4 ))
    [ $idx -lt 0 ] || [ $idx -ge ${#TARGETS[@]} ] && continue
    
    LABEL=${TARGETS[$idx]}
    TRIPLE=${TARGETS[$idx+1]}
    EXT=${TARGETS[$idx+2]}
    OS_DIR=${TARGETS[$idx+3]}

    # Update platform trackers for Apple deployment
    [[ "$TRIPLE" == "aarch64-apple-darwin" ]] && MACOS_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios-sim" ]] && IOS_SIM_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios" ]] && [[ "$TRIPLE" != *"-sim"* ]] && IOS_DEV_BUILT=true
    
    echo -e "\n${BLUE}${BOLD}🔨 Building $LABEL ($TRIPLE)...${NC}"
    rustup target add "$TRIPLE" > /dev/null 2>&1
    
    cargo build --target "$TRIPLE" --release
    
    if [ $? -eq 0 ]; then
        for l_choice in $lang_choices; do
            L_IDX=$((l_choice - 1))
            LANG=${LANGS[$L_IDX]}
            [[ "$LANG" == "swift" ]] && SWIFT_SELECTED=true
            
            if [ -n "$LANG" ]; then
                # Platform specific folder logic
                TARGET_OUT="./$OS_DIR/$LANG"
                echo -e "${YELLOW}📦 Generating $LANG bindings in $TARGET_OUT...${NC}"
                mkdir -p "$TARGET_OUT"
                
                LIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.${EXT}"
                if [ ! -f "$LIB_PATH" ]; then
                    LIB_PATH="./target/release/lib${LIB_NAME}.${EXT}"
                fi

                if [ -f "$LIB_PATH" ]; then
                    # --- FIX: Point UniFFI to the .dylib representation instead of the static archive (.a) to stop the metadata panic ---
                    BINDGEN_INPUT_PATH="$LIB_PATH"
                    if [[ "$OS_DIR" == "macOS" || "$OS_DIR" == "iOS" ]]; then
                        DYLIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.dylib"
                        if [ -f "$DYLIB_PATH" ]; then
                            BINDGEN_INPUT_PATH="$DYLIB_PATH"
                        fi
                    fi

                    if [[ "$LANG" == "swift" ]]; then
                        cargo run --bin uniffi-bindgen generate "$BINDGEN_INPUT_PATH" --language "$LANG" --config ./uniffi.toml --out-dir "$TARGET_OUT"
                        
                        # --- FIX: Inject 'nonisolated(unsafe)' to silence the Swift 6 vtable Concurrency Error ---
                        if [ -f "$TARGET_OUT/${LIB_NAME}.swift" ]; then
                            echo -e "${YELLOW}🩹 Patching generated Swift file for Swift 6 Concurrency compatibility...${NC}"
                            sed -i '' 's/static let vtablePtr/nonisolated\(unsafe\) static let vtablePtr/g' "$TARGET_OUT/${LIB_NAME}.swift"
                        fi
                    else
                        cargo run --bin uniffi-bindgen generate "$BINDGEN_INPUT_PATH" --language "$LANG" --out-dir "$TARGET_OUT"
                    fi

                    # For non-Apple targets, move the binary to the OS folder too
                    if [[ "$OS_DIR" != "macOS" && "$OS_DIR" != "iOS" ]]; then
                        cp "$LIB_PATH" "$TARGET_OUT/"
                    fi
                else
                    echo -e "${RED}❌ Error: lib${LIB_NAME}.${EXT} not found.${NC}"
                fi
            fi
        done
    fi
done

# --- UNIVERSAL APPLE DEPLOYMENT ---
if [ "$SWIFT_SELECTED" = true ]; then
    echo -e "\n${MAGENTA}${BOLD}🍎 Creating Unified XCFramework...${NC}"
    
    # Identify bridge source (prioritize Mac silicon headers)
    SWIFT_SOURCE_DIR="./macOS/swift"
    [ ! -d "$SWIFT_SOURCE_DIR" ] && SWIFT_SOURCE_DIR="./iOS/swift"
    
    if [ -d "$SWIFT_SOURCE_DIR" ]; then
        # Standardize modulemap for Xcode
        if [ -f "$SWIFT_SOURCE_DIR/${LIB_NAME}FFI.modulemap" ]; then
            cp "$SWIFT_SOURCE_DIR/${LIB_NAME}FFI.modulemap" "$SWIFT_SOURCE_DIR/module.modulemap"
        fi

        # Setup Framework output
        FRAMEWORK_DIR="./macOS/Frameworks"
        mkdir -p "$FRAMEWORK_DIR"
        rm -rf "$FRAMEWORK_DIR/${LIB_NAME}.xcframework"

        XCB_ARGS=""
        [ "$MACOS_BUILT" = true ] && XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-darwin/release/lib${LIB_NAME}.a -headers $SWIFT_SOURCE_DIR"
        [ "$IOS_SIM_BUILT" = true ] && XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-ios-sim/release/lib${LIB_NAME}.a -headers $SWIFT_SOURCE_DIR"
        [ "$IOS_DEV_BUILT" = true ] && XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-ios/release/lib${LIB_NAME}.a -headers $SWIFT_SOURCE_DIR"

        if [ -n "$XCB_ARGS" ]; then
            xcodebuild -create-xcframework $XCB_ARGS -output "$FRAMEWORK_DIR/${LIB_NAME}.xcframework"
            
            if [ -d "$SWIFT_PKG_DIR" ]; then
                echo -e "\n${YELLOW}🚚 Depositing into Swift Package: $SWIFT_PKG_DIR${NC}"
                mkdir -p "$SWIFT_PKG_DIR/Sources/XbibleEngine"
                cp -r "$FRAMEWORK_DIR/${LIB_NAME}.xcframework" "$SWIFT_PKG_DIR/"
                cp "$SWIFT_SOURCE_DIR/${LIB_NAME}.swift" "$SWIFT_PKG_DIR/Sources/XbibleEngine/"
                (cd "$SWIFT_PKG_DIR" && swift package clean)
                echo -e "${GREEN}${BOLD}✅ Universal Package Ready!${NC}"
            fi
        fi
    fi
fi
