PYTHON = python
CONVERT = convert
TTX = ttx

SRC = \
	Unison \
	latin greek cyrillic armenian \
	num num-roman runic \
	comb punct arrow box currency ctrl \
	math letterlike enclosed \
	hangul hangul-syllable \
	han han-radical \
	halfwidth fullwidth \
	yijing braille

SRCFILES = $(SRC:%=font/%.txt)

.PHONY: all
all: sample.html live.html sample.png unison.ttf

.PHONY: sample
sample: sample.html live.html sample.png

.PHONY: clean
clean:
	-$(RM) -f sample.html live.html sample.png sample.pgm unison.ttf unison.ttx

sample.html live.html sample.pgm unison.ttx: src/process.py $(SRCFILES)
	$(PYTHON) src/process.py $(SRCFILES)
sample.png: sample.pgm
	$(CONVERT) $< $@
unison.ttf: unison.ttx
	$(TTX) -f -o $@ $<
