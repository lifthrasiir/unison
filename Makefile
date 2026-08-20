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
# Brotli level for the WOFF2 outputs. `max` is what the published files are
# compressed with; the default trades 13% of their size for a second and a half
# per face, which is the right way round for a local edit-build loop that never
# serves them. See `render::Woff2Quality` for the measurements.
WOFF2QUALITY = fast

# The web takes one file per face. A WOFF2 collection is not usable from CSS —
# a browser has no way to select a face inside one — and `build` refuses it.
WOFF2 = $(FACES:%=unison-%.woff2)

OUTPUTS = demo.html sample.html live.html sample.png $(TTC) $(WOFF2)

.PHONY: all
all: $(OUTPUTS)

# `cargo test` only ever builds the default feature set, so the headless build
# rots silently: most of the crate is behind `#[cfg(feature = "editor")]`, and a
# test that reaches past that boundary compiles fine until someone thinks to try
# this. It has broken twice that way. A second debug profile, so it shares no
# target directory with the release build above and the two can run under `-j`.
.PHONY: check-headless
check-headless:
	$(CARGO) test --no-default-features

.PHONY: test
test: all check-headless $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- test -i $(INPATH)

.PHONY: clean
clean:
	-$(RM) -f $(OUTPUTS)

# The sample, preview, PNG and demo page all show the *first* declared face.
# `demo.html` is what the other three are being folded into: it embeds the
# primary face twice — the bitmap build and the vector build — and draws every
# specimen from the font itself rather than from pre-rendered SVG, which is why
# it is a fraction of sample.html's size. The three older outputs stay until it
# covers what they do.
$(OUTPUTS): $(INPATH)/*.unf $(SRC)
	$(CARGO) run $(CARGOFLAGS) -- build -i $(INPATH) -o $(TTC) -o unison-%.woff2 --woff2-quality $(WOFF2QUALITY) --sample-html sample.html --sample-png sample.png --live-html live.html \
	    --demo-html demo.html -d data
