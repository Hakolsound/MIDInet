PI_HOST  ?= pi@midinet.local
PI_TARGET = aarch64-unknown-linux-gnu

.PHONY: build build-pi test deploy provision clean

# ── Local development ─────────────────────────────────────────
build:
	cargo build --release

test:
	cargo test --workspace

clean:
	cargo clean

# ── Cross-compile for Raspberry Pi ────────────────────────────
build-pi:
	cargo build --release --target $(PI_TARGET)

# ── Deploy pre-built binaries via SCP ─────────────────────────
deploy: build-pi
	scp target/$(PI_TARGET)/release/midi-host  $(PI_HOST):/tmp/
	scp target/$(PI_TARGET)/release/midi-admin $(PI_HOST):/tmp/
	scp target/$(PI_TARGET)/release/midi-cli   $(PI_HOST):/tmp/
	ssh $(PI_HOST) 'sudo install -m 755 /tmp/midi-host /tmp/midi-admin /tmp/midi-cli /usr/local/bin/ && sudo systemctl restart midinet-host midinet-admin'

# ── First-time Pi setup (runs provision script on the Pi) ─────
provision:
	ssh $(PI_HOST) 'curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/pi-provision.sh | sudo bash'

# ── Trigger remote update (Pi pulls from git & rebuilds) ──────
update:
	ssh $(PI_HOST) 'sudo midinet-update'

# ── View live logs from Pi ────────────────────────────────────
logs:
	ssh $(PI_HOST) 'journalctl -u midinet-host -u midinet-admin -f'

status:
	ssh $(PI_HOST) 'systemctl status midinet-host midinet-admin --no-pager'
