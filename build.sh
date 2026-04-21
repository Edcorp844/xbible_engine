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
OUT_DIR="./bindings"
SWIFT_PKG_DIR="../Bible_engine_swift" 

# --- Target Data ---
# Note: For Apple targets, we use "a" (static lib) for XCFramework compatibility
TARGETS=(
    "macOS (Intel)"        "x86_64-apple-darwin"      "a"
    "macOS (Silicon)"      "aarch64-apple-darwin"    "a"
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
    
    # Static build for Apple platforms
    cargo build --target "$TRIPLE" --release
    
    if [ $? -eq 0 ]; then
        for l_choice in $lang_choices; do
            L_IDX=$((l_choice - 1))
            LANG=${LANGS[$L_IDX]}
            [[ "$LANG" == "swift" ]] && SWIFT_SELECTED=true
            
            if [ -n "$LANG" ]; then
                echo -e "${YELLOW}📦 Generating $LANG bindings...${NC}"
                mkdir -p "$OUT_DIR/$LANG"
                
                # Check for the library file (might be in target root or triple folder)
                LIB_PATH="./target/$TRIPLE/release/lib${LIB_NAME}.${EXT}"
                if [ ! -f "$LIB_PATH" ]; then
                    LIB_PATH="./target/release/lib${LIB_NAME}.${EXT}"
                fi

                if [ -f "$LIB_PATH" ]; then
                    cargo run --bin uniffi-bindgen generate --library "$LIB_PATH" --language "$LANG" --out-dir "$OUT_DIR/$LANG"
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
    
    SWIFT_BIND_DIR="$OUT_DIR/swift"
    
    # Standardize modulemap name for Xcode
    if [ -f "$SWIFT_BIND_DIR/${LIB_NAME}FFI.modulemap" ]; then
        mv "$SWIFT_BIND_DIR/${LIB_NAME}FFI.modulemap" "$SWIFT_BIND_DIR/module.modulemap"
    fi

    # Clean up previous framework
    rm -rf "${LIB_NAME}.xcframework"

    # Initialize XCFramework Arguments
    XCB_ARGS=""

    # 1. Add macOS (M1/Silicon)
    if [ "$MACOS_BUILT" = true ]; then
        MAC_LIB="./target/aarch64-apple-darwin/release/lib${LIB_NAME}.a"
        if [ -f "$MAC_LIB" ]; then
            XCB_ARGS="$XCB_ARGS -library $MAC_LIB -headers $SWIFT_BIND_DIR"
            echo -e "${CYAN}✓ Linked macOS (Silicon)${NC}"
        fi
    fi

    # 2. Add iOS Simulator
    if [ "$IOS_SIM_BUILT" = true ]; then
        SIM_LIB="./target/aarch64-apple-ios-sim/release/lib${LIB_NAME}.a"
        if [ -f "$SIM_LIB" ]; then
            XCB_ARGS="$XCB_ARGS -library $SIM_LIB -headers $SWIFT_BIND_DIR"
            echo -e "${CYAN}✓ Linked iOS Simulator${NC}"
        fi
    fi

    # 3. Add iOS Device
    if [ "$IOS_DEV_BUILT" = true ]; then
        DEV_LIB="./target/aarch64-apple-ios/release/lib${LIB_NAME}.a"
        if [ -f "$DEV_LIB" ]; then
            XCB_ARGS="$XCB_ARGS -library $DEV_LIB -headers $SWIFT_BIND_DIR"
            echo -e "${CYAN}✓ Linked iOS Device${NC}"
        fi
    fi

    # Execute xcodebuild if we have at least one library
    if [ -n "$XCB_ARGS" ]; then
        xcodebuild -create-xcframework $XCB_ARGS -output "${LIB_NAME}.xcframework"
        
        echo -e "\n${YELLOW}🚚 Depositing into Swift Package: $SWIFT_PKG_DIR${NC}"
        mkdir -p "$SWIFT_PKG_DIR/Sources/XbibleEngine" # Folder name must match target
        
        # Copy the framework and the swift bridge
        cp -r "${LIB_NAME}.xcframework" "$SWIFT_PKG_DIR/"
        cp "$SWIFT_BIND_DIR/${LIB_NAME}.swift" "$SWIFT_PKG_DIR/Sources/XbibleEngine/"
        
        # Re-sync the local package
        (cd "$SWIFT_PKG_DIR" && swift package clean)
        echo -e "${GREEN}${BOLD}✅ Universal Package Ready! Import $SWIFT_PKG_DIR in Xcode.${NC}"
    else
        echo -e "${RED}❌ No Apple targets built. Cannot create XCFramework.${NC}"
    fi
fi