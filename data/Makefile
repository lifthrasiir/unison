.PHONY: all
all: udhr_xml.zip UnicodeData.txt confusables.txt

.PHONY: clean
clean:
	-rm -f udhr_xml.zip UnicodeData.txt

udhr_xml.zip:
	curl -O --compressed http://unicode.org/udhr/assemblies/udhr_xml.zip

UnicodeData.txt:
	curl -O --compressed http://www.unicode.org/Public/UCD/latest/ucd/UnicodeData.txt

confusables.txt:
	curl -O --compressed http://www.unicode.org/Public/security/latest/confusables.txt

