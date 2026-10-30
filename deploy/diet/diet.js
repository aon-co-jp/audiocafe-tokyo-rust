(function () {
  var els = document.querySelectorAll('.diet-src');
  if (!els.length) return;
  fetch('/diet-recipes.json', { cache: 'no-cache' }).then(function (r) { return r.json(); }).then(function (d) {
    els.forEach(function (el) {
      var s = d.sources && d.sources[el.getAttribute('data-key')];
      if (!s || !s.latest) return;
      [['最新 / Latest', s.latest], ['一つ前 / Previous', s.previous]].forEach(function (p) {
        if (!p[1]) return;
        var div = document.createElement('div');
        div.style.cssText = 'font-size:1rem;font-weight:400;margin-top:.3rem;';
        div.appendChild(document.createTextNode(p[0] + '(' + p[1].date + '): '));
        var a = document.createElement('a');
        a.href = p[1].url; a.target = '_blank'; a.rel = 'noopener noreferrer';
        a.textContent = p[1].title;
        div.appendChild(a);
        el.appendChild(div);
      });
    });
  }).catch(function () {});
})();
