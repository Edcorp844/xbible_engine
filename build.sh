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
LIB_NAME="Bible_engine" 
OUT_DIR="./bindings"
SWIFT_PKG_DIR="../Bible_engine_swift" # Standardized directory name

# --- Target Data ---
TARGETS=(
    "macOS (Intel)"   "x86_64-apple-darwin"      "dylib"
    "macOS (Silicon)" "aarch64-apple-darwin"    "dylib"
    "iOS (Sim)"       "aarch64-apple-ios-sim"   "a"
    "iOS (Device)"    "aarch64-apple-ios"       "a"
    "Android"         "aarch64-linux-android"   "so"
)

# --- Language Data ---
LANGS=("swift" "kotlin" "csharp" "java")

echo -e "${MAGENTA}${BOLD}=======================================${NC}"
echo -e "${MAGENTA}${BOLD}    Bible_engine: Universal Build      ${NC}"
echo -e "${MAGENTA}${BOLD}=======================================${NC}"

# Step 1: Select Platform(s)
echo -e "${YELLOW}1. Select Target Platforms (e.g., 1 2):${NC}"
for ((i=0; i<${#TARGETS[@]}/3; i++)); do
    echo -e "${CYAN}$((i+1)))${NC} ${TARGETS[i*3]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r plat_choices

# Step 2: Select Language(s)
echo -e "\n${YELLOW}2. Select Binding Languages (e.g., 1):${NC}"
for i in "${!LANGS[@]}"; do
    echo -e "${CYAN}$((i+1)))${NC} ${LANGS[$i]}"
done
echo -n -e "${BOLD}Selection: ${NC}"
read -r lang_choices

# --- Execution ---
APPLE_SELECTED=false
SWIFT_SELECTED=false

for p_choice in $plat_choices; do
    idx=$(( (p_choice - 1) * 3 ))
    if [ $idx -lt 0 ] || [ $idx -ge ${#TARGETS[@]} ]; then continue; fi
    
    LABEL=${TARGETS[$idx]}
    TRIPLE=${TARGETS[$idx+1]}
    EXT=${TARGETS[$idx+2]}

    if [[ "$TRIPLE" == *"apple"* ]]; then APPLE_SELECTED=true; fi
    
    echo -e "\n${BLUE}${BOLD}🔨 Building $LABEL ($TRIPLE)...${NC}"
    cargo build --target "$TRIPLE" --release
    
    if [ $? -eq 0 ]; then
        for l_choice in $lang_choices; do
            L_IDX=$((l_choice - 1))
            LANG=${LANGS[$L_IDX]}
            if [[ "$LANG" == "swift" ]]; then SWIFT_SELECTED=true; fi
            
            if [ -n "$LANG" ]; then
                echo -e "${YELLOW}📦 Generating $LANG bindings...${NC}"
                mkdir -p "$OUT_DIR/$LANG"
                
                cargo run --bin uniffi-bindgen generate \
                    --library "./target/$TRIPLE/release/lib${LIB_NAME}.${EXT}" \
                    --language "$LANG" \
                    --out-dir "$OUT_DIR/$LANG"
            fi
        done
    fi
done

# --- Apple/Swift Specific Workflow (XCFramework) ---
if [ "$APPLE_SELECTED" = true ] && [ "$SWIFT_SELECTED" = true ]; then
    echo -e "\n${MAGENTA}${BOLD}🍎 Preparing XCFramework for Bible_engine...${NC}"

    # Rename modulemap for Xcode (UniFFI adds FFI suffix to the modulemap file)
    if [ -f "$OUT_DIR/swift/${LIB_NAME}FFI.modulemap" ]; then
        mv "$OUT_DIR/swift/${LIB_NAME}FFI.modulemap" "$OUT_DIR/swift/module.modulemap"
    fi

    rm -rf ios/Bible_engine.xcframework

    # Creating the XCFramework using the release static libs
    xcodebuild -create-xcframework \
        -library "./target/aarch64-apple-ios-sim/release/lib${LIB_NAME}.a" -headers "$OUT_DIR/swift" \
        -library "./target/aarch64-apple-ios/release/lib${LIB_NAME}.a" -headers "$OUT_DIR/swift" \
        -output "ios/Bible_engine.xcframework"

    echo -e "${YELLOW}Copying files to Swift package...${NC}"
    mkdir -p "$SWIFT_PKG_DIR/Sources/Bible_engine"
    cp -r ios/Bible_engine.xcframework "$SWIFT_PKG_DIR/"
    cp "$OUT_DIR/swift/${LIB_NAME}.swift" "$SWIFT_PKG_DIR/Sources/Bible_engine/"

    echo -e "${CYAN}Cleaning Swift Package...${NC}"
    if [ -d "$SWIFT_PKG_DIR" ]; then
        (cd "$SWIFT_PKG_DIR" && swift package clean)
    fi
    
    echo -e "${GREEN}${BOLD}✅ iOS/macOS files generated and copied!${NC}"
fi

echo -e "\n${GREEN}${BOLD}Workflow Complete for Bible_engine.${NC}"