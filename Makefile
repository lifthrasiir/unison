CARGO = cargo
CARGOFLAGS = -r

SRC = Cargo.* src/*.rs src/*/*.rs
INPATH = font

.PHONY: all
all: sample.html live.html sample.png unison.ttf unison.woff2

.PHONY: test
test: all $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- test -i $(INPATH)

.PHONY: clean
clean:
	-$(RM) -f sample.html live.html sample.png unison.ttf unison.woff2

sample.html live.html sample.png unison.ttf unison.woff2: $(INPATH)/*.unf $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- build -i $(INPATH) -o unison.ttf -o unison.woff2 --sample-html sample.html --sample-png sample.png --live-html live.html -d data
