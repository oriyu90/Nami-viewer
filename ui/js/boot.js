/* NamiVision boot — classic synchronous script.
 * Reads the tiny UI preference cache before the stylesheets paint so the
 * theme, language and UI scale do not flash. Refined by load_settings later.
 */
(function () {
  var root = document.documentElement;
  var prefs = null;
  try {
    var raw = localStorage.getItem('namivision.ui');
    if (raw) prefs = JSON.parse(raw);
  } catch (e) {
    prefs = null;
  }
  var theme = prefs && prefs.theme === 'light' ? 'light' : 'dark';
  var scale = prefs && Number.isFinite(Number(prefs.scale)) ? Number(prefs.scale) : 1;
  if (!(scale >= 0.8 && scale <= 1.5)) scale = 1;
  var lang = prefs && prefs.lang === 'en' ? 'en' : 'ja';
  root.setAttribute('data-theme', theme);
  root.style.colorScheme = theme;
  root.style.setProperty('--scale', String(scale));
  root.style.fontSize = 14 * scale + 'px';
  root.lang = lang;
})();
