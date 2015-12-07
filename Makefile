PYTHON = python
SRC = $(wildcard font/*.txt)

.PHONY: all
all: sample.html live.html sample.png unison.ttf

.PHONY: clean
clean:
	-rm -f sample.html live.html sample.png sample.pgm unison.ttf unison.ttx

sample.html live.html sample.pgm unison.ttx: src/process.py $(SRC)
	$(PYTHON) src/process.py $(SRC)
sample.png: sample.pgm
	convert $< $@
unison.ttf: unison.ttx
	ttx -f -o $@ $<
