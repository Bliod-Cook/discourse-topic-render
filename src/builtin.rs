pub const BUILTIN_CSS: &str = include_str!("builtin.css");

pub const THEME_TOGGLE_JS: &str = r#"(function () {
  var storageKey = "dtr-theme";
  var root = document.documentElement;
  var button = document.getElementById("dtr-theme-toggle");

  function preferredTheme() {
    try {
      return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
    } catch (_) {
      return "light";
    }
  }

  function effectiveTheme() {
    return root.getAttribute("data-theme") || preferredTheme();
  }

  function updateButton() {
    if (!button) return;
    var current = effectiveTheme();
    var next = current === "dark" ? "light" : "dark";
    button.textContent = next === "dark" ? "Dark" : "Light";
    button.setAttribute("aria-label", "Switch to " + next + " theme");
    button.setAttribute("title", "Switch to " + next + " theme");
  }

  function apply(theme) {
    if (theme === "light" || theme === "dark") {
      root.setAttribute("data-theme", theme);
    } else {
      root.removeAttribute("data-theme");
    }
    updateButton();
  }

  var saved = null;
  try {
    saved = localStorage.getItem(storageKey);
  } catch (_) {
    saved = null;
  }
  apply(saved);

  if (button) {
    button.addEventListener("click", function () {
      var next = effectiveTheme() === "dark" ? "light" : "dark";
      try {
        localStorage.setItem(storageKey, next);
      } catch (_) {}
      apply(next);
    });
  }
})();"#;
