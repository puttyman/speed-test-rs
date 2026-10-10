// Registers the service worker (SPEC §7.9). Dependency-free; loaded with `defer`.
//
// - Only registers in a secure context (HTTPS or localhost). On plain http://<LAN IP> the
//   app works normally without offline support.
// - When a new worker is installed and waiting, dispatches `pwa:update-available` on
//   `window` and sets `window.__pwaUpdateAvailable = true` (for listeners attached later,
//   e.g. after hydration).
// - `window.__pwaApplyUpdate()` tells the waiting worker to take over, then reloads once
//   the new worker controls the page. Returns false when there is nothing to apply.
(function () {
  "use strict";

  var waitingWorker = null;
  var reloading = false;

  window.__pwaUpdateAvailable = false;
  window.__pwaApplyUpdate = function () {
    if (!waitingWorker) return false;
    navigator.serviceWorker.addEventListener("controllerchange", function () {
      if (reloading) return;
      reloading = true;
      window.location.reload();
    });
    waitingWorker.postMessage({ type: "SKIP_WAITING" });
    return true;
  };

  if (!("serviceWorker" in navigator) || !window.isSecureContext) return;

  function announce(worker) {
    waitingWorker = worker;
    window.__pwaUpdateAvailable = true;
    window.dispatchEvent(new CustomEvent("pwa:update-available", { detail: { worker: worker } }));
  }

  function watch(registration) {
    // A worker was already waiting when the page loaded.
    if (registration.waiting && navigator.serviceWorker.controller) {
      announce(registration.waiting);
    }
    registration.addEventListener("updatefound", function () {
      var worker = registration.installing;
      if (!worker) return;
      worker.addEventListener("statechange", function () {
        // `controller` is null on the very first install: nothing to update then.
        if (worker.state === "installed" && navigator.serviceWorker.controller) {
          announce(worker);
        }
      });
    });
  }

  function register() {
    navigator.serviceWorker
      .register("/sw.js", { scope: "/" })
      .then(watch)
      .catch(function (err) {
        console.warn("speed-test-rs: service worker registration failed", err);
      });
  }

  if (document.readyState === "complete") {
    register();
  } else {
    window.addEventListener("load", register);
  }
})();
