BIN      := btpad-autodisconnect
BINDIR   := $(HOME)/.local/bin
SVCDIR   := $(HOME)/.config/systemd/user
SVCFILE  := $(BIN).service

.PHONY: build install uninstall

build:
	cargo build --release

install: build
	install -Dm755 target/release/$(BIN) $(BINDIR)/$(BIN)
	install -Dm644 $(SVCFILE) $(SVCDIR)/$(SVCFILE)
	systemctl --user daemon-reload
	systemctl --user enable --now $(BIN)

uninstall:
	systemctl --user disable --now $(BIN) || true
	rm -f $(BINDIR)/$(BIN) $(SVCDIR)/$(SVCFILE)
	systemctl --user daemon-reload
