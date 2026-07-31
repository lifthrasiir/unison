CARGO = cargo
CARGOFLAGS = -r

SRC = Cargo.* src/*.rs src/*/*.rs
INPATH = font

# One file per `face` in font/Unison.unf. Kept explicit rather than derived:
# make cannot ask the font what faces it declares, and a stale name here fails
# loudly instead of silently shipping one face.
FACES = regular term

# Desktop and terminal installs take the collection: the faces share every
# table but `cmap` and `name`, so one file is about half the size of the two
# fonts separately.
TTC = unison.ttc
# The web takes one file per face. A WOFF2 collection is not usable from CSS —
# a browser has no way to select a face inside one — and `build` refuses it.
WOFF2 = $(FACES:%=unison-%.woff2)

OUTPUTS = sample.html live.html sample.png $(TTC) $(WOFF2)

.PHONY: all
all: $(OUTPUTS)

.PHONY: test
test: all $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- test -i $(INPATH)

.PHONY: clean
clean:
	-$(RM) -f $(OUTPUTS)

# The sample, preview and PNG show the *first* declared face.
$(OUTPUTS): $(INPATH)/*.unf $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- build -i $(INPATH) -o $(TTC) -o unison-%.woff2 --sample-html sample.html --sample-png sample.png --live-html live.html -d data
