if exists("b:did_ftplugin")
  finish
endif

let b:did_ftplugin = 1

setl comments=://
setl iskeyword+=-
setl list
setl tabstop=8
setl noexpandtab

let b:undo_ftplugin = "setlocal comments< iskeyword< list< tabstop< expandtab<"
