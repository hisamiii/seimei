// Small Markdown renderer for seimei's replies.
// Everything is escaped first; only the tags built here can appear, so raw HTML in a reply never runs.

const ESC = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
export const esc = (s) => s.replace(/[&<>"']/g, (c) => ESC[c]);

const link = (href, text) => `<a href="${esc(href)}" target="_blank" rel="noopener noreferrer">${text}</a>`;

function inline(src) {
  const slots = [];
  const put = (html) => `\u0000${slots.push(html) - 1}\u0000`;
  let s = src.replace(/`([^`\n]+)`/g, (_, c) => put(`<code>${esc(c)}</code>`));
  s = s.replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, t, u) => put(link(u, esc(t))));
  s = s.replace(/https?:\/\/[^\s<>()]*[^\s<>().,;:!?'"]/g, (u) => put(link(u, esc(u))));
  s = esc(s)
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^\w*])\*([^*\n]+)\*(?![\w*])/g, '$1<em>$2</em>')
    .replace(/(^|\W)_([^_\n]+)_(?!\w)/g, '$1<em>$2</em>')
    .replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
  return s.replace(/\u0000(\d+)\u0000/g, (_, i) => slots[i]);
}

const FENCE = /^\s*(```|~~~)/;
const ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
const indent = (l) => l.match(/^\s*/)[0].length;
const isTable = (lines, i) => lines[i].includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]);
const startsBlock = (lines, i) =>
  FENCE.test(lines[i]) || /^#{1,6}\s/.test(lines[i]) || lines[i].startsWith('>') ||
  ITEM.test(lines[i]) || RULE.test(lines[i]) || isTable(lines, i);

function cells(row) {
  return row.trim().replace(/^\|/, '').replace(/\|$/, '').split(/(?<!\\)\|/).map((c) => inline(c.trim()));
}

function list(lines, i) {
  const base = indent(lines[i]);
  const ordered = (l) => /^\s*\d/.test(l);
  const tag = ordered(lines[i]) ? 'ol' : 'ul';
  const sameList = (l) => ITEM.test(l) && indent(l) === base && ordered(l) === (tag === 'ol');
  const items = [];
  while (i < lines.length) {
    const l = lines[i];
    const m = l.match(ITEM);
    if (m && m[1].length === base) {
      if (!sameList(l)) break;
      items.push([m[3]]);
    } else if (!l.trim()) {
      const next = lines[i + 1];
      if (next === undefined || !(indent(next) > base || sameList(next))) break;
    } else if (indent(l) > base && items.length) {
      items[items.length - 1].push(l.replace(new RegExp(`^\\s{0,${base + 4}}`), ''));
    } else {
      break;
    }
    i++;
  }
  const lis = items.map(([first, ...rest]) => `<li>${inline(first)}${rest.length ? md(rest.join('\n')) : ''}</li>`);
  return [`<${tag}>${lis.join('')}</${tag}>`, i];
}

export function md(src) {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    let m;
    if (!line.trim()) {
      i++;
    } else if ((m = line.match(FENCE))) {
      const body = [];
      for (i++; i < lines.length && !lines[i].trim().startsWith(m[1]); i++) body.push(lines[i]);
      i++;
      out.push(`<pre><code>${esc(body.join('\n'))}</code></pre>`);
    } else if ((m = line.match(/^(#{1,6})\s+(.*)$/))) {
      const level = Math.min(m[1].length + 2, 6);
      out.push(`<h${level}>${inline(m[2])}</h${level}>`);
      i++;
    } else if (RULE.test(line)) {
      out.push('<hr>');
      i++;
    } else if (line.startsWith('>')) {
      const body = [];
      while (i < lines.length && lines[i].startsWith('>')) body.push(lines[i++].replace(/^>\s?/, ''));
      out.push(`<blockquote>${md(body.join('\n'))}</blockquote>`);
    } else if (isTable(lines, i)) {
      const head = cells(lines[i]);
      const rows = [];
      for (i += 2; i < lines.length && lines[i].includes('|') && lines[i].trim(); i++) rows.push(cells(lines[i]));
      const tr = (cs, t) => `<tr>${cs.map((c) => `<${t}>${c}</${t}>`).join('')}</tr>`;
      out.push(`<div class="table"><table><thead>${tr(head, 'th')}</thead><tbody>${rows.map((r) => tr(r, 'td')).join('')}</tbody></table></div>`);
    } else if (ITEM.test(line)) {
      const [html, next] = list(lines, i);
      out.push(html);
      i = next;
    } else {
      const para = [line];
      for (i++; i < lines.length && lines[i].trim() && !startsBlock(lines, i); i++) para.push(lines[i]);
      out.push(`<p>${para.map(inline).join('<br>')}</p>`);
    }
  }
  return out.join('');
}
