# Demo video

`demo.tape` films a real session in a clean container:

1. `npm install -g @tokenade/cli`
2. `tokenade install`
3. Claude Code runs a noisy `pytest -v` in a small project (`project/`); Tokenade folds the passing-test lines
4. the terminal dashboard shows the tokens saved in that session

Every number on screen comes from that session. The container has a neutral
user (`dev`), project path (`/demo`) and prompt, and `IS_DEMO=1` hides the
Claude Code account.

Regenerate (needs Docker and a Claude Code login on the host):

    docs/demo/record.sh [output-dir]   # default: docs/assets

It writes `demo.mp4` (1080p, H.264, no audio) and `demo.gif`. The agent's
answer differs from one take to the next; the tape waits on screen text rather
than fixed delays, so the video stays around 40–50 s.
