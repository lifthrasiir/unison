PYTHON = python
CONVERT = convert
TTX = ttx
CARGO = cargo
CARGOFLAGS =

SRC = \
	Unison \
	latin greek cyrillic armenian \
	num num-roman runic \
	comb punct arrow box currency ctrl \
	math letterlike enclosed \
	hangul hangul-syllable \
	han han-radical \
	halfwidth fullwidth \
	yijing braille playing-card

SRCFILES = $(SRC:%=font/%.txt)

.PHONY: all
all: sample.html live.html sample.png unison.ttf

.PHONY: sample
sample: sample.html live.html sample.png

.PHONY: clean
clean:
	-$(RM) -f .ran_process .ran_cargo sample.html live.html sample.png sample.pgm unison.json unison.ttf unison.ttx

unison.json unison.ttx: .ran_process
.ran_process: src/process.py $(SRCFILES)
	$(PYTHON) src/process.py $(SRCFILES)
	@touch $@

sample.html live.html sample.pgm: .ran_cargo
.ran_cargo: unison.json Cargo.toml src/*.rs
	$(CARGO) run $(CARGOFLAGS) < $<
	@touch $@

sample.png: sample.pgm
	$(CONVERT) $< -define png:bit-depth=2 -define png:color-type=3 $@

unison.ttf: unison.ttx
	$(TTX) -f -o $@ $<

