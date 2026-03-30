# Open Issues

## 1. HTML rich text stored raw (e.g. Slack copies)

**Symptom:** Copying a link or formatted text from Slack stores the raw HTML
(`<a href="mailto:...">text</a>`) instead of the readable plain text.

**Root cause:** Slack (and many Electron apps) write `text/html` to the
clipboard alongside `text/plain`. The monitor currently captures whichever
MIME type arboard returns first, which is often `text/html`.

**Fix plan:**
- In `monitor.rs`: prefer `text/plain` when the clipboard has both types.
- If only `text/html` is available, strip tags before storing so history
  shows clean text.
- Store the stripped plain text with `mime_type = "text/plain"` so paste
  always inserts readable content.

## 2. Pasting into Claude Code CLI says "no image found to paste"

**Symptom:** After selecting a history item and pressing Enter (or
Super+Shift+V), pasting into the Claude Code terminal fails with
"no image found to paste".

**Root cause:** When the clipboard entry has `mime_type = "text/html"`,
`PasteEngine::set_clipboard` writes it with the `text/html` MIME type.
The Claude Code CLI interprets a `/paste` or Ctrl+V with a non-plain-text
MIME type as an image paste attempt and rejects it.

**Fix plan:**
- In `paste.rs`: when MIME type is `text/html`, always downcast to
  `text/plain` and strip tags before writing to the clipboard / injecting.
- This ensures terminal apps always receive clean plain text.
