/**
 * Keeps the app in the part of the screen that the on-screen keyboard leaves free.
 *
 * Phone web views draw the keyboard over the page instead of resizing it (Android with
 * edge-to-edge, and iOS), so fields and buttons at the bottom would end up behind it. The
 * visual viewport does shrink, so the app takes its height from there. `data-keyboard` on the
 * root drops the bottom safe-area padding while the keyboard covers that area anyway.
 */
export function followVisualViewport() {
  const viewport = window.visualViewport;
  if (!viewport) return;
  const root = document.documentElement;

  const update = () => {
    // Unscaled, so that pinch zoom in a desktop browser does not shrink the app.
    const height = viewport.height * viewport.scale;
    root.style.setProperty("--viewport-height", `${Math.round(height)}px`);
    root.toggleAttribute("data-keyboard", window.innerHeight - height > 100);
    // Web views scroll the page to reveal a focused field; the app is already sized to fit.
    if (window.scrollY !== 0) window.scrollTo(0, 0);
  };

  viewport.addEventListener("resize", update);
  viewport.addEventListener("scroll", update);
  update();
}
