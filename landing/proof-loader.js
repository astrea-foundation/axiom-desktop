// Only this tiny entry loads with the page. The card renderer, SVG parser and
// geometry builders are downloaded when the reader approaches the chapter.
const chapter = document.querySelector('#proof');
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');

if (chapter && !reducedMotion.matches && !location.search.includes('static')) {
  if ('IntersectionObserver' in window) {
    const preload = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      preload.disconnect();
      import('./proof-scene.js').catch((error) => {
        chapter.querySelectorAll('.card').forEach((card) => card.classList.add('in'));
        console.warn('The proof chapter is using its readable card fallback.', error);
      });
    }, { rootMargin: '1600px' });
    preload.observe(chapter);
  } else {
    chapter.querySelectorAll('.card').forEach((card) => card.classList.add('in'));
  }
}
