#!/usr/bin/env bash
set -euo pipefail

ELF="$1"
HEX="${ELF%.elf}.hex"

avr-objcopy -O ihex -R .eeprom "$ELF" "$HEX"
avrdude -c usbasp -p t45 -U flash:w:"$HEX":i
