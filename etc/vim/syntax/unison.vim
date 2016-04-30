if exists("b:current_syntax")
  finish
endif

let s:cpo_save = &cpo
set cpo&vim

syn match unisonCommand "\S\@<!\(glyph\|map\|remap\|feature\|exclude-from-sample\|default\|name-parts\)\S\@!"
syn match unisonInlineCommand "\S\@<!\(inline\|sticky\|point\|pixel\)\S\@!"

syn match unisonContinueToken "\S\@<!\.\.\(\s\+\/\/\|\s*$\)\@="
syn match unisonSpecialToken "\S\@<![:=]\S\@!"
syn match unisonSeparator "[()|]\|->"

syn match unisonComment "\S\@<!//.*$"
syn region unisonQuoted start="`" skip="``" end="`"

syn match unisonPixelWhite "[*@]" contained
syn match unisonPixelBlack "[.]" contained
syn match unisonPixelMostlyWhite "[dbP9un()]" contained
syn match unisonPixelMostlyBlack "[/\\^v<>]" contained
syn match unisonPixelInherited "!" contained
syn match unisonPixelUnknown "?" contained
syn match unisonPixelOutside "+" contained
syn match unisonPixelA "A" contained
syn match unisonPixelB "B" contained
syn match unisonPixelC "C" contained
syn match unisonPixelD "D" contained
syn match unisonPixelE "E" contained
syn match unisonPixelF "F" contained
syn match unisonPixelG "G" contained
syn match unisonPixelH "H" contained
syn match unisonPixelI "I" contained
syn match unisonPixelJ "J" contained
syn match unisonPixelK "K" contained
syn match unisonPixelL "L" contained
syn match unisonPixelM "M" contained
syn match unisonPixelN "N" contained
syn match unisonPixelO "O" contained
syn match unisonPixelQ "Q" contained
syn match unisonPixelR "R" contained
syn match unisonPixelS "S" contained
syn match unisonPixelT "T" contained
syn match unisonPixelU "U" contained
syn match unisonPixelV "V" contained
syn match unisonPixelW "W" contained
syn match unisonPixelX "X" contained
syn match unisonPixelY "Y" contained
syn match unisonPixelZ "Z" contained
syn match unisonPattern "^\([a-z-]\+\(\s\|$\)\|//\)\@!\S\+" contains=unisonPixel.*

syn match unisonCodepoint "\S\@<!U+[0-9a-fA-F]\{4,8}\(\.\.[0-9a-fA-F]\{4,8}\)\?\(/[0-9a-fA-F]\+\)\?\S\@!" contains=unisonCodepointSep
syn match unisonCodepointSep "\.\.\|/" contained
syn match unisonNameParts "\$[0-9a-zA-Z\-_.]\+"
syn match unisonSign "\(\S\|^\)\@<![+-]\S"me=e-1
syn match unisonRepeat "\*\d\+[|)]"me=e-1 contains=unisonRepeatCount
syn match unisonRepeatCount "\d\+" contained
syn match unisonSpecifier "\(\S\@<![0-9a-zA-Z\-_.()|$]\+\)\@<=[@!]\S\+" contains=unisonPositionalSpecifier,unisonFilteringSpecifier
syn match unisonPositionalSpecifier "@\([<v>^]\+\|[A-Z]\{1,2}\)"ms=s+1 contained contains=unisonPixel[A-Z]
syn match unisonFilteringSpecifier "!\(negate\)"ms=s+1 contained
" TODO long pixel names

hi def link unisonCommand Keyword
hi def link unisonInlineCommand Keyword

hi def link unisonContinueToken PreProc
hi def link unisonSpecialToken SpecialChar
hi def link unisonSeparator Delimiter

hi def link unisonComment Comment
hi def link unisonQuoted String

hi def link unisonCodepoint Character
hi def link unisonCodepointSep Delimiter
hi def link unisonNameParts Identifier
hi def link unisonSign Special
hi def link unisonRepeat Special
hi def link unisonRepeatCount Number
hi def link unisonSpecifier Special
hi def link unisonPositionalSpecifier Constant
hi def link unisonFilteringSpecifier Identifier

hi def unisonPattern          ctermbg=black guibg=black
hi def unisonPixelWhite       ctermbg=black guibg=black ctermfg=white guifg=#ffffff
hi def unisonPixelMostlyWhite ctermbg=black guibg=black ctermfg=white guifg=#bbbbbb
hi def unisonPixelMostlyBlack ctermbg=black guibg=black ctermfg=gray guifg=#777777
hi def unisonPixelBlack	      ctermbg=black guibg=black ctermfg=darkgray guifg=#333333
hi def unisonPixelInherited   ctermbg=black guibg=black ctermfg=yellow guifg=yellow
hi def unisonPixelUnknown     ctermbg=black guibg=black ctermfg=magenta guifg=magenta
hi def unisonPixelOutside     ctermbg=gray guibg=gray ctermfg=black guifg=black

" try to assign distant colors for consecutive letters
" steps 7 letters for each 360/25 degrees of hue; ignore P for the pixel
hi def unisonPixelA ctermbg=darkred     guibg=#800000 ctermfg=black guifg=gray
hi def unisonPixelH ctermbg=darkred     guibg=#801e00 ctermfg=black guifg=gray
hi def unisonPixelO ctermbg=darkyellow  guibg=#803d00 ctermfg=black guifg=gray
hi def unisonPixelW ctermbg=darkyellow  guibg=#805c00 ctermfg=black guifg=gray
hi def unisonPixelD ctermbg=darkyellow  guibg=#807a00 ctermfg=black guifg=gray
hi def unisonPixelK ctermbg=darkyellow  guibg=#668000 ctermfg=black guifg=gray
hi def unisonPixelS ctermbg=darkgreen   guibg=#478000 ctermfg=black guifg=gray
hi def unisonPixelZ ctermbg=darkgreen   guibg=#288000 ctermfg=black guifg=gray
hi def unisonPixelG ctermbg=darkgreen   guibg=#0a8000 ctermfg=black guifg=gray
hi def unisonPixelN ctermbg=darkgreen   guibg=#008014 ctermfg=black guifg=gray
hi def unisonPixelV ctermbg=darkcyan    guibg=#008033 ctermfg=black guifg=gray
hi def unisonPixelC ctermbg=darkcyan    guibg=#008051 ctermfg=black guifg=gray
hi def unisonPixelJ ctermbg=darkcyan    guibg=#008070 ctermfg=black guifg=gray
hi def unisonPixelR ctermbg=darkcyan    guibg=#007080 ctermfg=black guifg=gray
hi def unisonPixelY ctermbg=darkblue    guibg=#005180 ctermfg=black guifg=gray
hi def unisonPixelF ctermbg=darkblue    guibg=#003380 ctermfg=black guifg=gray
hi def unisonPixelM ctermbg=darkblue    guibg=#001480 ctermfg=black guifg=gray
hi def unisonPixelU ctermbg=darkblue    guibg=#0a0080 ctermfg=black guifg=gray
hi def unisonPixelB ctermbg=darkblue    guibg=#280080 ctermfg=black guifg=gray
hi def unisonPixelI ctermbg=darkmagenta guibg=#470080 ctermfg=black guifg=gray
hi def unisonPixelQ ctermbg=darkmagenta guibg=#660080 ctermfg=black guifg=gray
hi def unisonPixelX ctermbg=darkmagenta guibg=#80007a ctermfg=black guifg=gray
hi def unisonPixelE ctermbg=darkmagenta guibg=#80005c ctermfg=black guifg=gray
hi def unisonPixelL ctermbg=darkred     guibg=#80003d ctermfg=black guifg=gray
hi def unisonPixelT ctermbg=darkred     guibg=#80001e ctermfg=black guifg=gray

let b:current_syntax = "unison"

let &cpo = s:cpo_save
unlet s:cpo_save

" vim: set sw=2 sts=2 ts=8 noet:
