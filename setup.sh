#!/bin/bash

# --- Colors ---
BLUE='\033[0;34m'
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color
BOLD='\033[1m'

echo -e "${CYAN}${BOLD}=======================================${NC}"
echo -e "${CYAN}${BOLD}    xBible Engine: Rust Environment    ${NC}"
echo -e "${CYAN}${BOLD}=======================================${NC}"
echo -e "${YELLOW}Please select the targets you wish to add:${NC}"
echo -e "Use ${GREEN}y${NC} to install or ${RED}n${NC} to skip."
echo "---------------------------------------"

# Function to handle the install
install_target() {
    local platform_name=$1
    local target_string=$2
    
    read -p "$(echo -e ${BOLD}"Install $platform_name targets? (y/n): "${NC})" choice
    if [[ "$choice" == "y" || "$choice" == "Y" ]]; then
        echo -e "${BLUE}Adding targets for $platform_name...${NC}"
        for target in $target_string; do
            rustup target add "$target"
            if [ $? -eq 0 ]; then
                echo -e "  ${GREEN}✓${NC} Added $target"
            else
                echo -e "  ${RED}✗${NC} Failed to add $target"
            fi
        done
    else
        echo -e "${YELLOW}Skipped $platform_name.${NC}"
    fi
    echo "---------------------------------------"
}

# --- Target Definitions ---

# 1. Apple / Mac
install_target "macOS" "x86_64-apple-darwin aarch64-apple-darwin"

# 2. Linux
install_target "Linux" "x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"

# 3. Android
install_target "Android" "aarch64-linux-android armv7-linux-androideabi x86_64-linux-android"

# 4. Windows
install_target "Windows" "x86_64-pc-windows-msvc i686-pc-windows-msvc"

echo -e "\n${GREEN}${BOLD}Setup process complete for xBible!${NC}"
echo -e "To verify, run: ${CYAN}rustup target list --installed${NC}"