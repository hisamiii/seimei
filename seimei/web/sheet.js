// One bottom sheet, filled with a shikigami, one of the user's sessions, or the activity log.
import { $ } from './util.js';

export const sheet = { wrap: $('sheet-wrap'), body: $('sheet-body'), view: null, worker: null, log: null, renderer: null, head: null };

export function showSheet(view) {
  sheet.view = view;
  if (view !== 'worker') {
    sheet.worker = null;
    sheet.renderer = null;
  }
  sheet.body.classList.toggle('scroll', view !== 'worker');
  if (sheet.wrap.hidden) {
    sheet.wrap.hidden = false;
    history.pushState({ sheet: true }, '');
  }
}

export function closeSheet(fromHistory = false) {
  if (sheet.wrap.hidden) return;
  sheet.wrap.hidden = true;
  sheet.view = null;
  sheet.worker = null;
  sheet.renderer = null;
  if (!fromHistory && history.state?.sheet) history.back();
}

$('scrim').addEventListener('click', () => closeSheet());
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') closeSheet(); });
