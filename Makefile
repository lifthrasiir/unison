PYTHON = python
CONVERT = convert
TTX = ttx

SRC = \
	Unison \
	latin greek cyrillic armenian \
	num num-roman \
	comb punct arrow box currency ctrl \
	hangul hangul-syllable han \
	halfwidth fullwidth \
	yijing braille

SRCFILES = $(SRC:%=font/%.txt)

.PHONY: all
all: sample.html live.html sample.png unison.ttf

.PHONY: clean
clean:
	-$(RM) -f sample.html live.html sample.png sample.pgm unison.ttf unison.ttx

sample.html live.html sample.pgm unison.ttx: src/process.py $(SRCFILES)
	$(PYTHON) src/process.py $(SRCFILES)
sample.png: sample.pgm
	$(CONVERT) $< $@
unison.ttf: unison.ttx
	$(TTX) -f -o $@ $<
