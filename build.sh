#!/bin/bash

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
OUT_DIR="./bindings"
SWIFT_PKG_DIR="../Bible_engine_swift" 

# --- Target Data ---
TARGETS=(
    "macOS (Intel)"        "x86_64-apple-darwin"      "dylib"
    "macOS (Silicon)"      "aarch64-apple-darwin"    "dylib"
    "iOS (Sim)"            "aarch64-apple-ios-sim"   "a"
    "iOS (Device)"         "aarch64-apple-ios"       "a"
    "Android (ARM64)"      "aarch64-linux-android"   "so"
    "Android (x86_64/Sim)" "x86_64-linux-android"    "so"
    "Linux (x86_64)"       "x86_64-unknown-linux-gnu" "so"
    "Windows (x86_64)"     "x86_64-pc-windows-msvc"  "dll"
)

LANGS=("swift" "kotlin" "csharp" "java" "c" "cpp" "python" "ruby")

echo -e "${MAGENTA}${BOLD}=======================================${NC}"
echo -e "${MAGENTA}${BOLD}    xbible_engine: Universal Build     ${NC}"
echo -e "${MAGENTA}${BOLD}=======================================${NC}"

# Step 1: Select Platform(s)
echo -e "${YELLOW}1. Select Target Platforms (Recommended: 2 3 4):${NC}"
for ((i=0; i<${#TARGETS[@]}/3; i++)); do
    echo -e "${CYAN}$((i+1)))${NC} ${TARGETS[i*3]}"
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
    idx=$(( (p_choice - 1) * 3 ))
    [ $idx -lt 0 ] || [ $idx -ge ${#TARGETS[@]} ] && continue
    
    LABEL=${TARGETS[$idx]}
    TRIPLE=${TARGETS[$idx+1]}
    EXT=${TARGETS[$idx+2]}

    # Update platform trackers
    [[ "$TRIPLE" == "aarch64-apple-darwin" ]] && MACOS_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios-sim" ]] && IOS_SIM_BUILT=true
    [[ "$TRIPLE" == "aarch64-apple-ios" ]] && [[ "$TRIPLE" != *"-sim"* ]] && IOS_DEV_BUILT=true
    
    echo -e "\n${BLUE}${BOLD}🔨 Building $LABEL ($TRIPLE)...${NC}"
    rustup target add "$TRIPLE" > /dev/null 2>&1
    
    # We build staticlibs (.a) for XCFramework for better stability
    cargo build --target "$TRIPLE" --release
    
    if [ $? -eq 0 ]; then
        for l_choice in $lang_choices; do
            L_IDX=$((l_choice - 1))
            LANG=${LANGS[$L_IDX]}
            [[ "$LANG" == "swift" ]] && SWIFT_SELECTED=true
            
            if [ -n "$LANG" ]; then
                echo -e "${YELLOW}📦 Generating $LANG bindings...${NC}"
                mkdir -p "$OUT_DIR/$LANG"
                LIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.${EXT}"
                [ ! -f "$LIB_PATH" ] && LIB_PATH="./target/release/lib${LIB_NAME}.${EXT}"

                if [ -f "$LIB_PATH" ]; then
                    cargo run --bin uniffi-bindgen generate --library "$LIB_PATH" --language "$LANG" --out-dir "$OUT_DIR/$LANG"
                fi
            fi
        done
    fi
done

# --- UNIVERSAL APPLE DEPLOYMENT ---
if [ "$SWIFT_SELECTED" = true ]; then
    echo -e "\n${MAGENTA}${BOLD}🍎 Creating Unified XCFramework...${NC}"
    
    SWIFT_BIND_DIR="$OUT_DIR/swift"
    [ -f "$SWIFT_BIND_DIR/${LIB_NAME}FFI.modulemap" ] && mv "$SWIFT_BIND_DIR/${LIB_NAME}FFI.modulemap" "$SWIFT_BIND_DIR/module.modulemap"

    mkdir -p ios
    rm -rf ios/${LIB_NAME}.xcframework

    # Initialize XCFramework Arguments
    XCB_ARGS=""

    # Add macOS if built
    if [ "$MACOS_BUILT" = true ]; then
        # Note: For XCFrameworks, using the static library (.a) is often preferred over .dylib
        MAC_LIB="./target/aarch64-apple-darwin/release/lib${LIB_NAME}.a"
        [ ! -f "$MAC_LIB" ] && MAC_LIB="./target/release/lib${LIB_NAME}.a"
        if [ -f "$MAC_LIB" ]; then
            XCB_ARGS="$XCB_ARGS -library $MAC_LIB -headers $SWIFT_BIND_DIR"
            echo -e "${CYAN}Added macOS to bundle.${NC}"
        fi
    fi

    # Add iOS Simulator if built
    if [ "$IOS_SIM_BUILT" = true ]; then
        XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-ios-sim/release/lib${LIB_NAME}.a -headers $SWIFT_BIND_DIR"
        echo -e "${CYAN}Added iOS Simulator to bundle.${NC}"
    fi

    # Add iOS Device if built
    if [ "$IOS_DEV_BUILT" = true ]; then
        XCB_ARGS="$XCB_ARGS -library ./target/aarch64-apple-ios/release/lib${LIB_NAME}.a -headers $SWIFT_BIND_DIR"
        echo -e "${CYAN}Added iOS Device to bundle.${NC}"
    fi

    # Create the XCFramework only if we have libraries to bundle
    if [ -n "$XCB_ARGS" ]; then
        xcodebuild -create-xcframework $XCB_ARGS -output "ios/${LIB_NAME}.xcframework"
        
        echo -e "${YELLOW}Depositing into Swift Package...${NC}"
        mkdir -p "$SWIFT_PKG_DIR/Sources/Bible_engine"
        cp -r ios/${LIB_NAME}.xcframework "$SWIFT_PKG_DIR/"
        cp "$SWIFT_BIND_DIR/${LIB_NAME}.swift" "$SWIFT_PKG_DIR/Sources/Bible_engine/"
        
        (cd "$SWIFT_PKG_DIR" && swift package clean)
        echo -e "${GREEN}${BOLD}✅ Universal Package Ready for local import!${NC}"
    else
        echo -e "${RED}❌ No Apple libraries found to bundle. Did you select targets 2, 3, or 4?${NC}"
    fi
fi