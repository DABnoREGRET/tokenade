# Tokenade demo — regenerate with docs/demo/record.sh (see README.md there).
Output "/out/demo.mp4"
Output "/out/demo.gif"

Set Shell "bash"
Set FontSize 22
Set Width 1920
Set Height 1080
Set Padding 36
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set WindowBar Colorful
Set WindowBarSize 44

Hide
Hide
Type "clear" Enter
Show
Show

Type "npm install -g @tokenade/cli" Sleep 400ms Enter
Wait+Screen@120s /added \d+ package/
Sleep 1.2s

Type "tokenade install" Sleep 400ms Enter
Wait+Line@60s /^\$\s*$/
Sleep 3s

Hide
Type "clear" Enter
Show
Type "claude --model sonnet" Sleep 400ms Enter
Wait+Screen@60s /❯/
Sleep 1s
Type "Run python3 -m pytest -v exactly as written (no head or tail) and tell me which tests fail and why. Don't fix anything." Sleep 500ms Enter
Wait+Screen@240s /for \d+s/
Sleep 7s
Ctrl+C
Sleep 300ms
Ctrl+C
Wait+Line@20s /^\$\s*$/

Hide
Type "clear" Enter
Show
# A container has no browser, so the report is the terminal one, paged.
Type "tokenade dashboard --text | less" Sleep 400ms Enter
Wait+Screen@60s /TOP COMPACTORS/
Sleep 8s
Type "q"
Sleep 1s
