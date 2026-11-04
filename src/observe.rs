use serde::Deserialize;

pub const MAX_OBSERVATION_CHARS: usize = 12_000;
#[derive(Debug, Deserialize)]
pub struct Element {
    pub r#ref: u32,
    pub line: String,
    pub selector: String,
}
#[derive(Debug, Deserialize)]
pub struct Observation {
    pub url: String,
    pub title: String,
    pub elements: Vec<Element>,
    pub text: String,
}
pub fn render(o: &Observation) -> String {
    let mut rendered = format!("URL: {}\nTitle: {}\n", o.url, o.title);
    rendered.push_str(
        &o.elements
            .iter()
            .map(|e| e.line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    rendered.push_str("\nText:\n");
    rendered.push_str(&o.text);
    if rendered.chars().count() > MAX_OBSERVATION_CHARS {
        const MARKER: &str = "…[truncated]";
        let end = rendered
            .char_indices()
            .nth(MAX_OBSERVATION_CHARS - MARKER.chars().count())
            .unwrap()
            .0;
        rendered.truncate(end);
        rendered.push_str(MARKER);
    }
    rendered
}

pub(crate) const OBSERVE_JS: &str = r###"(() => {
    const collapse = s => String(s || '').replace(/\s+/gu, ' ').trim();
    const limit = (s, n) => Array.from(s).slice(0, n).join('');
    // Field values may be mirrored in page labels, titles or body text.
    const values = [];
    for (const e of document.querySelectorAll('input,textarea,[contenteditable]')) {
        if (e.matches('input') && ['checkbox','radio','button','submit','reset','image'].includes(e.type)) continue;
        values.push(e.value || '', e.defaultValue || '', e.hasAttribute('contenteditable') ? e.innerText : '');
    }
    const sensitive = Array.from(new Set(values.flatMap(v => [v, collapse(v)])))
        .filter(Boolean).sort((a,b) => b.length-a.length);
    const scrub = s => sensitive.reduce((text, v) => text.split(v).join('••••'), String(s || ''));
    const visible = e => {
        const r = e.getBoundingClientRect(), style = getComputedStyle(e);
        return r.width > 0 && r.height > 0 && style.display !== 'none'
            && style.visibility !== 'hidden' && style.visibility !== 'collapse';
    };
    const unique = sel => document.querySelectorAll(sel).length === 1;
    const idSelector = e => e.id ? '#' + CSS.escape(e.id) : null;
    const selector = e => {
        const id = idSelector(e);
        if (id && unique(id)) return id;
        const parts = [];
        for (let node = e; node; node = node.parentElement) {
            const anchor = idSelector(node);
            if (node !== e && anchor && unique(anchor)) {
                const sel = anchor + ' > ' + parts.join(' > ');
                if (unique(sel)) return sel;
            }
            if (node === document.body) break;
            const siblings = Array.from(node.parentElement.children).filter(s => s.localName === node.localName);
            parts.unshift(node.localName + ':nth-of-type(' + (siblings.indexOf(node) + 1) + ')');
        }
        const sel = 'body' + (parts.length ? ' > ' + parts.join(' > ') : '');
        if (!unique(sel)) throw new Error('could not produce unique selector');
        return sel;
    };
    // Read names without using form-control values or editable contents.
    const nameText = (e, allowHidden = false) => {
        if (!e || (!allowHidden && !visible(e)) || e.matches('input,textarea,select,[contenteditable]')) return '';
        return Array.from(e.childNodes).map(n => n.nodeType === Node.TEXT_NODE
            ? n.textContent : n.nodeType === Node.ELEMENT_NODE ? nameText(n, allowHidden) : '').join(' ');
    };
    const name = e => {
        const labelled = (e.getAttribute('aria-labelledby') || '').split(/\s+/u)
            .map(id => nameText(document.getElementById(id), true)).join(' ');
        return limit(scrub(collapse(labelled || e.getAttribute('aria-label')
            || (e.labels ? Array.from(e.labels).map(label => nameText(label)).join(' ') : '')
            || e.getAttribute('alt') || nameText(e) || e.getAttribute('title'))), 80);
    };
    const role = e => {
        if (e.getAttribute('role')) return e.getAttribute('role');
        switch (e.localName) {
            case 'a': return 'link';
            case 'button': return 'button';
            case 'select': return 'combobox';
            case 'textarea': return 'textbox';
            case 'input':
                if (['checkbox', 'radio'].includes(e.type)) return e.type;
                if (['button', 'submit', 'reset', 'image'].includes(e.type)) return 'button';
                if (e.type === 'range') return 'slider';
                if (e.type === 'number') return 'spinbutton';
                return 'textbox';
            case 'h1': case 'h2': case 'h3': return 'heading';
            default: return 'textbox';
        }
    };
    const included = 'a[href],button,input,select,textarea,[role="button"],[role="link"],[role="checkbox"],[role="tab"],[role="menuitem"],[contenteditable],h1,h2,h3';
    const elements = [];
    for (const e of document.querySelectorAll(included)) {
        if (!visible(e)) continue;
        const ref = elements.length + 1;
        let line = '[' + ref + '] ' + role(e) + ' ' + JSON.stringify(name(e));
        if (e.matches('a[href]')) {
            try { line += ' → ' + scrub(new URL(e.getAttribute('href'), document.baseURI).pathname); }
            catch { /* An invalid href has no path to describe. */ }
        }
        if (e.localName === 'input') {
            line += ' type=' + e.type;
            if (e.placeholder) line += ' placeholder=' + JSON.stringify(limit(scrub(collapse(e.placeholder)), 80));
            line += e.value ? ' filled' : ' empty';
        }
        if (e.localName === 'select') {
            line += ' options=' + JSON.stringify(Array.from(e.options).map(o => limit(scrub(collapse(o.label)), 80)));
        }
        elements.push({ref, line, selector: selector(e)});
        if (elements.length === 200) break;
    }
    // innerText can include textarea defaults and editable contents.
    const text = document.body ? document.body.innerText : '';
    return {url: scrub(location.href), title: scrub(document.title), elements,
        text: limit(collapse(scrub(text)), 4000)};
})()
"###;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_truncates_and_formats() {
        let mut o = Observation {
            url: "https://example.com/form".into(),
            title: "Form".into(),
            elements: vec![Element {
                r#ref: 1,
                line: "[1] button \"Go\"".into(),
                selector: "#go".into(),
            }],
            text: "Ready".into(),
        };
        assert_eq!(
            render(&o),
            "URL: https://example.com/form\nTitle: Form\n[1] button \"Go\"\nText:\nReady"
        );
        o.text = "界".repeat(12_000);
        let rendered = render(&o);
        assert_eq!(rendered.chars().count(), 12_000);
        assert!(rendered.ends_with("…[truncated]"));
        let prefix = "URL: https://example.com/form\nTitle: Form\n[1] button \"Go\"\nText:\n";
        o.text = "x".repeat(12_000 - prefix.chars().count());
        assert_eq!(render(&o), format!("{prefix}{}", o.text));
    }
}
