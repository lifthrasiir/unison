CARGO = cargo
CARGOFLAGS = -r

INPATH = font

.PHONY: all
all: sample.html live.html sample.png unison.ttf unison.woff2

.PHONY: clean
clean:
	-$(RM) -f sample.html live.html sample.png unison.ttf unison.woff2

sample.html live.html sample.png unison.ttf unison.woff2: $(INPATH)/*.unf
	$(CARGO) run $(CARGOFLAGS) -- build -i $(INPATH) -o unison.ttf -o unison.woff2 --sample-html sample.html --sample-png sample.png --live-html live.html -d data
