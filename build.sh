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
    "Android (ARM64)"      "aarch64-linux-android"   "so"  "android"
    "Android (x86_64/Sim)" "x86_64-linux-android"    "so"  "android"
    "Linux (x86_64)"       "x86_64-unknown-linux-gnu" "so" "linux"
    "Windows (x86_64)"     "x86_64-pc-windows-msvc"  "dll" "lindows"
)

LANGS=("swift" "kotlin" "csharp" "java" "c" "cpp" "python" "ruby")

echo -e "${MAGENTA}${BOLD}=======================================${NC}"
echo -e "${MAGENTA}${BOLD}    xbible_engine: Universal Build     ${NC}"
echo -e "${MAGENTA}${BOLD}=======================================${NC}"

# Step 1: Select Platform(s)
echo -e "${YELLOW}1. Select Target Platforms (Choosing 1, 2, 3, or 4 builds ALL Apple platforms):${NC}"
for ((i=0; i<${#TARGETS[@]}/4; i++)); do
    echo -e "${CYAN}$((i+1)))${NC} ${TARGETS[i*4]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r plat_choices

# Automatically expand choices if any Apple platform is selected
if [[ " $plat_choices " =~ " 1 " || " $plat_choices " =~ " 2 " || " $plat_choices " =~ " 3 " || " $plat_choices " =~ " 4 " ]]; then
    echo -e "${YELLOW}🔄 Expanding selection to build ALL Apple architectures together...${NC}"
    # Merge 1, 2, 3, 4 into choices safely while keeping other non-apple flags
    plat_choices="1 2 3 4 $(echo "$plat_choices" | sed 's/[1234]//g')"
fi

# Step 2: Select Language(s)
echo -e "\n${YELLOW}2. Select Binding Languages:${NC}"
for i in "${!LANGS[@]}"; do
    echo -e "${CYAN}$((i+1)))${NC} ${LANGS[$i]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r lang_choices

# Trackers for XCFramework
SWIFT_SELECTED=false
MACOS_INTEL_BUILT=false
MACOS_SILICON_BUILT=false
IOS_SIM_BUILT=false
IOS_DEV_BUILT=false

for p_choice in $plat_choices; do
    idx=$(( (p_choice - 1) * 4 ))
    [ $idx -lt 0 ] || [ $idx -ge ${#TARGETS[@]} ] && continue
    
    LABEL=${TARGETS[$idx]}
    TRIPLE=${TARGETS[$idx+1]}
    EXT=${TARGETS[$idx+2]}
    OS_DIR=${TARGETS[$idx+3]}

    # Track target status
    [[ "$TRIPLE" == "x86_64-apple-darwin" ]] && MACOS_INTEL_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-darwin" ]] && MACOS_SILICON_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios-sim" ]] && IOS_SIM_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios" ]] && IOS_DEV_BUILT=true
    
    echo -e "\n${BLUE}${BOLD}🔨 Building $LABEL ($TRIPLE)...${NC}"
    rustup target add "$TRIPLE" > /dev/null 2>&1
    
    cargo build --target "$TRIPLE" --release
    
    if [ $? -eq 0 ]; then
        for l_choice in $lang_choices; do
            L_IDX=$((l_choice - 1))
            LANG=${LANGS[$L_IDX]}
            [[ "$LANG" == "swift" ]] && SWIFT_SELECTED=true
            
            if [ -n "$LANG" ]; then
                TARGET_OUT="./$OS_DIR/$LANG"
                echo -e "${YELLOW}📦 Generating $LANG bindings in $TARGET_OUT...${NC}"
                mkdir -p "$TARGET_OUT"
                
                LIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.${EXT}"
                if [ ! -f "$LIB_PATH" ]; then
                    LIB_PATH="./target/release/lib${LIB_NAME}.${EXT}"
                fi

                if [ -f "$LIB_PATH" ]; then
                    BINDGEN_INPUT_PATH="$LIB_PATH"
                    if [[ "$OS_DIR" == "macOS" || "$OS_DIR" == "iOS" ]]; then
                        DYLIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.dylib"
                        if [ -f "$DYLIB_PATH" ]; then
                            BINDGEN_INPUT_PATH="$DYLIB_PATH"
                        fi
                    fi

                    if [[ "$LANG" == "swift" ]]; then
                        cargo run --bin uniffi-bindgen generate "$BINDGEN_INPUT_PATH" --language "$LANG" --config ./uniffi.toml --out-dir "$TARGET_OUT"
                        
                        if [ -f "$TARGET_OUT/${LIB_NAME}.swift" ]; then
                            echo -e "${YELLOW}🩹 Patching generated Swift file for Swift 6 Concurrency compatibility...${NC}"
                            sed -i '' 's/static let vtablePtr/nonisolated\(unsafe\) static let vtablePtr/g' "$TARGET_OUT/${LIB_NAME}.swift" 2>/dev/null || sed -i 's/static let vtablePtr/nonisolated\(unsafe\) static let vtablePtr/g' "$TARGET_OUT/${LIB_NAME}.swift"
                        fi
                    else
                        cargo run --bin uniffi-bindgen generate "$BINDGEN_INPUT_PATH" --language "$LANG" --out-dir "$TARGET_OUT"
                    fi

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
    
    # Pick headers folder dynamically
    SWIFT_SOURCE_DIR="./macOS/swift"
    [ ! -d "$SWIFT_SOURCE_DIR" ] && SWIFT_SOURCE_DIR="./iOS/swift"
    
    if [ -d "$SWIFT_SOURCE_DIR" ]; then
        if [ -f "$SWIFT_SOURCE_DIR/${LIB_NAME}FFI.modulemap" ]; then
            cp "$SWIFT_SOURCE_DIR/${LIB_NAME}FFI.modulemap" "$SWIFT_SOURCE_DIR/module.modulemap"
        fi

        FRAMEWORK_DIR="./macOS/Frameworks"
        mkdir -p "$FRAMEWORK_DIR"
        rm -rf "$FRAMEWORK_DIR/${LIB_NAME}.xcframework"

        # --- CRITICAL LIPO STEP FOR MACOS MULTI-ARCH ---
        MAC_FAT_DIR="./target/macOS-fat"
        mkdir -p "$MAC_FAT_DIR"
        
        if [ "$MACOS_INTEL_BUILT" = true ] && [ "$MACOS_SILICON_BUILT" = true ]; then
            echo -e "${YELLOW}🔗 Lipo merging macOS Intel and Silicon binaries...${NC}"
            lipo -create \
                "./target/x86_64-apple-darwin/release/lib${LIB_NAME}.a" \
                "./target/aarch64-apple-darwin/release/lib${LIB_NAME}.a" \
                -output "$MAC_FAT_DIR/lib${LIB_NAME}.a"
            HAS_MAC_FAT=true
        fi

        # Assemble the Xcode command args dynamically
        XCB_ARGS=""
        if [ "$HAS_MAC_FAT" = true ]; then
            XCB_ARGS="$XCB_ARGS -library $MAC_FAT_DIR/lib${LIB_NAME}.a -headers $SWIFT_SOURCE_DIR"
        elif [ "$MACOS_SILICON_BUILT" = true ]; then
            XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-darwin/release/lib${LIB_NAME}.a -headers $SWIFT_SOURCE_DIR"
        fi

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
                echo -e "${GREEN}${BOLD}✅ Universal Cross-Platform Package Ready!${NC}"
            fi
        fi
    fi
fi