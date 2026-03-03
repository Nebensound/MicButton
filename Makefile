ELF = target/avr-attiny45/release/mic-button.elf
HEX = target/avr-attiny45/release/mic-button.hex

.PHONY: build flash test clean

build:
	cargo avr-build

flash: build
	avr-objcopy -O ihex -R .eeprom $(ELF) $(HEX)
	avrdude -c usbasp -p t45 -U flash:w:$(HEX):i

test:
	cargo test --lib

clean:
	cargo clean
