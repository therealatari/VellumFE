//! Text assembly: bold/output regions, text element creation, entity
//! decoding, event-pattern matching, GSL tag stripping, and attribute
//! extraction.

use super::*;

impl XmlParser {
    pub(super) fn handle_output(&mut self, tag: &str) {
        // <output class="mono"/> opens a monospace region (tables, ASCII
        // art - game XML, also emitted by Lich's respond() for script
        // output); <output class=""/> closes it. The tag itself renders
        // nothing.
        self.mono_output = Self::extract_attribute(tag, "class").as_deref() == Some("mono");
    }

    pub(super) fn handle_push_bold(&mut self) {
        // <pushBold/> is SEMANTIC markup — "this is a hostile creature" — not a
        // font instruction. Its entire visual meaning is the monsterbold color
        // preset (owner decision 2026-08-11). The stack tracks the open scope
        // for the preset pop, link color priority, and SpanType::Monsterbold;
        // it must never surface as a font-bold flag on text.
        self.bold_stack.push(true);

        // Apply monsterbold color preset
        if let Some((fg, bg)) = self.presets.get("monsterbold") {
            self.preset_stack.push(ColorStyle {
                fg: fg.clone(),
                bg: bg.clone(),
            });
        }
    }

    pub(super) fn handle_pop_bold(&mut self) {
        // <popBold/> - remove bold and color
        self.bold_stack.pop();

        // Remove monsterbold color if we added it
        if !self.preset_stack.is_empty() {
            self.preset_stack.pop();
        }
    }

    pub(super) fn create_text_element(&mut self, content: String) -> ParsedElement {
        // Get current colors from stacks (last pushed takes precedence)
        let mut fg = None;
        let mut bg = None;
        // NEVER derived from bold_stack: <pushBold> means "monsterbold style"
        // (color, carried by the preset stack and SpanType::Monsterbold below),
        // not "bolden the font". The segment-level bold flag is reserved for the
        // user's own highlight rules, which are applied later in the pipeline.
        let bold = false;

        // Check stacks in order: color > preset > style
        for style in &self.color_stack {
            if style.fg.is_some() {
                fg = style.fg.clone();
            }
            if style.bg.is_some() {
                bg = style.bg.clone();
            }
        }
        for style in &self.preset_stack {
            if fg.is_none() && style.fg.is_some() {
                fg = style.fg.clone();
            }
            if bg.is_none() && style.bg.is_some() {
                bg = style.bg.clone();
            }
        }
        for style in &self.style_stack {
            if fg.is_none() && style.fg.is_some() {
                fg = style.fg.clone();
            }
            if bg.is_none() && style.bg.is_some() {
                bg = style.bg.clone();
            }
        }

        // Decode HTML entities, then drop stray control characters the feed
        // occasionally leaks (keep \t; \n never reaches us — input is
        // line-framed). Matches the C0/DEL strip Saga applies at text ingress.
        let content = Self::strip_control_chars(Self::decode_entities(content));

        // If we're inside a link (<a> or <d> tag), append this text to the
        // OUTERMOST open link's text field — the one that wins and is
        // surfaced on the span — and keep the mirrored `current_link_data` in
        // sync so the span below clones the up-to-date text.
        if self.link_depth > 0 {
            if let Some(link_data) = self.link_stack.iter_mut().find(|d| !d.exist_id.is_empty()) {
                link_data.text.push_str(&content);
            }
            if let Some(ref mut mirror) = self.current_link_data {
                mirror.text.push_str(&content);
            }
        }

        // Determine semantic type based on current state
        // Priority: Monsterbold > Spell > Link > Speech > Normal
        let span_type = if !self.bold_stack.is_empty() {
            SpanType::Monsterbold
        } else if self.spell_depth > 0 {
            SpanType::Spell
        } else if self.link_depth > 0 {
            SpanType::Link
        } else if self.current_preset_id.as_deref() == Some("speech") {
            SpanType::Speech
        } else {
            SpanType::Normal
        };

        ParsedElement::Text {
            content,
            stream: self.current_stream.clone(),
            fg_color: fg,
            bg_color: bg,
            bold,
            mono: self.mono_output,
            span_type,
            link_data: self.current_link_data.clone(),
        }
    }

    /// Decode entities repeatedly until stable. Simu double-encodes some
    /// display titles ("Friends &amp;amp;&amp;amp; Enemies" on the wire),
    /// so a single pass leaves "&amp;" in the human string. Only for
    /// display-title attributes — game TEXT must decode exactly once, or a
    /// literal "&amp;" someone typed would collapse.
    pub(super) fn decode_entities_stable(mut text: String) -> String {
        loop {
            let decoded = Self::decode_entities(text.clone());
            if decoded == text {
                break text;
            }
            text = decoded;
        }
    }

    /// Extract the element name from a raw tag string like "<pushStream id=..>"
    /// or "</compDef>". Empty if the input is not tag-shaped.
    pub(super) fn tag_name(tag: &str) -> &str {
        let body = tag
            .strip_prefix("</")
            .or_else(|| tag.strip_prefix('<'))
            .unwrap_or(tag);
        let end = body
            .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
            .unwrap_or(body.len());
        &body[..end]
    }

    /// Every element name the Wrayth wire protocol is known to emit, whether
    /// or not we handle it. Union of our dispatch chain and the tag set the
    /// Saga client recognizes. Unknown names indicate new server markup and
    /// are passed through as visible text (never silently dropped) so
    /// protocol changes announce themselves. MUST stay sorted: binary search.
    pub(super) const KNOWN_WIRE_TAGS: &'static [&'static str] = &[
        "FEVersion", "LaunchURL", "LichWebUI", "PantheonStatus", "a", "annotate", "app", "b",
        "br", "castTime", "celebration", "checkBox", "clearContainer", "clearDynaStream",
        "clearStream", "cli", "closeButton", "closeDialog", "closedialog", "cmdButton",
        "cmdlist", "cmdtimestamp", "compDef", "compass", "component", "container",
        "continuation", "crtrStatus", "d", "deleteContainer", "description", "dialogData",
        "dir", "dropDownBox", "dynaStream", "editBox", "endSetup", "exists", "exits",
        "exposeContainer", "exposeDialog", "exposeStream", "extra", "flag", "forcesave",
        "getSkinVersion", "group", "hScrollBar", "hostile", "i", "image", "indicator", "inv",
        "inventoryManager", "inventoryViewItem", "label", "launchURL", "left", "link",
        "macros", "menu", "menuImage", "menuLink", "mi", "mode", "monopolize", "name", "nav",
        "nomenu", "noverbupdates", "objective", "objectives", "openDialog", "opendialog",
        "output", "palette", "playerID", "players", "popBold", "popInputState", "popStream",
        "popup", "preset", "presets", "progressBar", "prompt", "pulse", "pushBold",
        "pushInputState", "pushStream", "radio", "resource", "result", "right", "roomDesc",
        "roommeta", "roundTime", "sentSettings", "sep", "settings", "settingsInfo", "skin",
        "spell", "stream", "streamId", "streamWindow", "string", "switchQuickBar", "timer",
        "tipInfo", "upDownEditBox", "updateverbs", "vScrollBar", "worldEvent",
    ];

    pub(super) fn is_known_wire_tag(name: &str) -> bool {
        Self::KNOWN_WIRE_TAGS.binary_search(&name).is_ok()
    }

    /// Remove C0 control characters (except \t and \n) and DEL. The wire
    /// occasionally carries stray control bytes; rendering them corrupts
    /// terminal output. Fast path returns the string untouched.
    pub(super) fn strip_control_chars(text: String) -> String {
        if !text
            .bytes()
            .any(|b| (b < 0x20 && b != b'\t' && b != b'\n') || b == 0x7f)
        {
            return text;
        }
        text.chars()
            .filter(|&c| c == '\t' || c == '\n' || (c != '\u{7f}' && c >= '\u{20}'))
            .collect()
    }

    /// If `after` begins with a `<pushStream>` tag whose id matches the
    /// stream that is currently open, return that tag's length so the
    /// caller can skip the whole <popStream/><pushStream/> pair. Adjacency
    /// is required: any text between the tags belongs to the outer stream
    /// and means this is a real stream switch, not fragment glue.
    pub(super) fn same_stream_repush_len(after: &str, current_stream: &str) -> Option<usize> {
        if current_stream.is_empty() || current_stream == "main" {
            return None;
        }
        if !after.starts_with("<pushStream") {
            return None;
        }
        let tag_end = after.find('>')?;
        let tag = &after[..tag_end + 1];
        if Self::extract_attribute(tag, "id").as_deref() == Some(current_stream) {
            Some(tag_end + 1)
        } else {
            None
        }
    }

    /// Close-tag match that tolerates mangled trailing junk before the '>'.
    /// Game data occasionally ships broken escaping — e.g. ability HELP text
    /// with `$<a href=$Q...$>Recent Evasion$</a$>` — and a strict `"</a>"`
    /// comparison leaves the link style open, bleeding link color over
    /// everything after it. The name must end at a non-alphanumeric char so
    /// `</a$>` closes a link but `</app>` never does.
    pub(super) fn is_close_tag(tag: &str, name: &str) -> bool {
        tag.strip_prefix("</")
            .and_then(|rest| rest.strip_prefix(name))
            .is_some_and(|rest| {
                !rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            })
    }

    /// Decode a numeric character reference at the start of `rest`
    /// ("&#123;" or "&#x1F;"). Returns the character and the byte length
    /// consumed, or None if `rest` is not a well-formed numeric reference
    /// (in which case the caller passes the '&' through verbatim).
    fn decode_numeric_entity(rest: &str) -> Option<(char, usize)> {
        let body = rest.strip_prefix("&#")?;
        let semi = body.find(';').filter(|&p| p > 0 && p <= 8)?;
        let digits = &body[..semi];
        let value = if let Some(hex) = digits.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            digits.parse::<u32>().ok()?
        };
        // from_u32 rejects surrogates and out-of-range values
        let ch = char::from_u32(value)?;
        Some((ch, 2 + semi + 1))
    }

    pub(super) fn decode_entities(text: String) -> String {
        // Fast path: most game text has no entities at all
        if !text.contains('&') {
            return text;
        }
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            if text.as_bytes()[i] == b'&' {
                let rest = &text[i..];
                let (decoded, len) = if rest.starts_with("&lt;") {
                    ('<', 4)
                } else if rest.starts_with("&gt;") {
                    ('>', 4)
                } else if rest.starts_with("&amp;") {
                    ('&', 5)
                } else if rest.starts_with("&quot;") {
                    ('"', 6)
                } else if rest.starts_with("&apos;") {
                    ('\'', 6)
                } else if let Some((ch, len)) = Self::decode_numeric_entity(rest) {
                    (ch, len)
                } else {
                    // Unknown entity - copy the '&' through verbatim
                    out.push('&');
                    i += 1;
                    continue;
                };
                out.push(decoded);
                i += len;
            } else {
                // Copy everything up to the next '&' (or end) in one go
                let next = text[i..].find('&').map_or(text.len(), |p| i + p);
                out.push_str(&text[i..next]);
                i = next;
            }
        }
        out
    }

    /// Flush text buffer and check for event patterns
    pub(super) fn flush_text_with_events(
        &mut self,
        text: String,
        elements: &mut Vec<ParsedElement>,
    ) {
        if text.is_empty() {
            return;
        }

        // Check if we should auto-exit inventory stream
        // Inventory updates don't send <popStream/>, so we detect terminator lines
        if self.current_stream == "inv" {
            const INV_TERMINATORS: &[&str] = &[
                "You pick up",
                "You drop",
                "You retrieve",
                "You sheathe",
                "You draw",
                "You put",
            ];

            // Check if this line terminates the inventory stream
            for terminator in INV_TERMINATORS {
                if text.trim_start().starts_with(terminator) {
                    tracing::debug!(
                        "Detected inventory terminator: '{}' - switching to main stream",
                        terminator
                    );
                    self.current_stream = "main".to_string();
                    elements.push(ParsedElement::StreamPop);
                    break;
                }
            }
        }

        // Check for event patterns on the text
        let event_elements = self.check_event_patterns(&text);
        elements.extend(event_elements);

        // Add the text element itself
        elements.push(self.create_text_element(text));
    }

    /// Check text against event patterns and return any matching events
    pub(super) fn check_event_patterns(&self, text: &str) -> Vec<ParsedElement> {
        let mut events = Vec::new();

        for (regex, pattern) in &self.event_matchers {
            if let Some(captures) = regex.captures(text) {
                let mut duration = pattern.duration;

                // Extract duration from capture group if specified
                if let Some(group_idx) = pattern.duration_capture {
                    if let Some(capture) = captures.get(group_idx) {
                        if let Ok(captured_value) = capture.as_str().parse::<f32>() {
                            // Apply multiplier (e.g., rounds to seconds)
                            duration = (captured_value * pattern.duration_multiplier) as u32;
                        }
                    }
                }

                // tracing::debug!(
                //                     "Event pattern '{}' matched: '{}' (duration: {}s)",
                //                     pattern.pattern,
                //                     text,
                //                     duration
                //                 );

                events.push(ParsedElement::Event {
                    event_type: pattern.event_type.clone(),
                    action: pattern.action.clone(),
                    duration,
                });
            }
        }

        events
    }

    /// Check if a line is purely a GSL protocol tag (should be skipped entirely)
    ///
    /// Returns true for lines like "GSjBCDFGH" (compass) that are GSL control messages
    pub(super) fn is_gsl_tag_line(line: &str) -> bool {
        // Pattern: "GS" followed by a lowercase letter (byte peek - the
        // prefix is ASCII, so as_bytes indexing is safe and allocation-free)
        if line.starts_with("GS") && line.len() >= 3 && line.as_bytes()[2].is_ascii_lowercase() {
            return true;
        }
        // Also check for lines starting with \x1C (control char prefix)
        line.starts_with('\x1C')
    }

    /// Strip GSL (GemStone Language) protocol tags sent by Lich proxy
    ///
    /// Lich sends GSL control sequences for compass, status indicators, etc.
    /// These start with \x1C (File Separator) followed by "GS" + letter + data,
    /// OR appear as bare "GSx..." lines (where x is a letter like 'j' for compass)
    ///
    /// Examples:
    /// - "GSjBCDFGH" = compass directions (j=junctions, BCDFGH=encoded exits)
    /// - "GSg0000000050" = stance value
    /// - "GSP..." = prompt indicators
    /// - "\x1CGSB..." = character info with control char prefix
    pub(super) fn strip_gsl_tags(line: &str) -> std::borrow::Cow<'_, str> {
        // Handle lines that are purely GSL tags (no leading \x1C in logs)
        // Pattern: "GS" followed by a lowercase letter, then optional data
        if line.starts_with("GS") && line.len() >= 3 && line.as_bytes()[2].is_ascii_lowercase() {
            // This is a GSL tag line - filter it out entirely
            tracing::debug!("[GSL] Filtering GSL tag: '{}'", line);
            return std::borrow::Cow::Borrowed("");
        }

        // Handle embedded GSL tags with \x1C prefix: everything from the
        // first \x1C to end of line is GSL data (processed line by line).
        // The overwhelmingly common case is no \x1C at all - borrow as-is.
        match line.find('\x1C') {
            None => std::borrow::Cow::Borrowed(line),
            Some(pos) => std::borrow::Cow::Borrowed(&line[..pos]),
        }
    }

    /// Byte offset just past `attr=<quote>` in `tag`, if present. Byte-wise
    /// scan so the parser's hottest helper allocates nothing while searching.
    pub(super) fn find_attr_value_start(tag: &str, attr: &str, quote: u8) -> Option<usize> {
        let bytes = tag.as_bytes();
        let name = attr.as_bytes();
        let probe_len = name.len() + 2; // attr + '=' + quote
        if bytes.len() < probe_len {
            return None;
        }
        for i in 0..=bytes.len() - probe_len {
            // Attribute names always follow whitespace (and can never start
            // the tag); without the boundary check "exp=" would match inside
            // "field_exp='340'".
            if bytes[i..i + name.len()] == *name
                && bytes[i + name.len()] == b'='
                && bytes[i + name.len() + 1] == quote
                && i > 0
                && bytes[i - 1].is_ascii_whitespace()
            {
                return Some(i + probe_len);
            }
        }
        None
    }

    /// Extract every `name="value"` / `name='value'` pair from a tag, in
    /// order. Valueless attributes and malformed pairs are skipped.
    /// pub(crate) so the core layer can scan tags embedded in component
    /// values (which are captured raw).
    pub(crate) fn extract_all_attributes(tag: &str) -> Vec<(String, String)> {
        let mut attrs = Vec::new();
        let bytes = tag.as_bytes();
        let mut i = 0;
        // Skip past the tag name (up to the first whitespace)
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let name_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i == name_start {
                // Hit '/', '>', or end - no more attributes
                break;
            }
            let name = &tag[name_start..i];
            if i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                    let quote = bytes[i];
                    i += 1;
                    let value_start = i;
                    while i < bytes.len() && bytes[i] != quote {
                        i += 1;
                    }
                    if i < bytes.len() {
                        attrs.push((name.to_string(), tag[value_start..i].to_string()));
                        i += 1; // past closing quote
                    }
                }
            }
        }
        attrs
    }
}
