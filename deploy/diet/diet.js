(function () {
  var sl = document.getElementById('diet-search-list');
  if (sl) {
    fetch('/diet-recipes.json', { cache: 'no-cache' }).then(function (r) { return r.json(); }).then(function (d) {
      var rs = d.search && d.search.results;
      if (!rs || rs.length < 3) return;
      while (sl.firstChild) sl.removeChild(sl.firstChild);
      rs.forEach(function (x) {
        var li = document.createElement('li');
        li.style.cssText = 'margin:0 0 .9rem;';
        var a = document.createElement('a');
        a.href = x.url; a.target = '_blank'; a.rel = 'noopener noreferrer'; a.style.fontWeight = '700'; a.textContent = x.title;
        li.appendChild(a);
        function line(t, en, small) {
          if (!t) return;
          li.appendChild(document.createElement('br'));
          var sp = document.createElement('span');
          if (en) sp.lang = 'en';
          sp.style.cssText = (small ? 'font-size:.95rem;' : 'font-weight:700;') + (en && small ? 'opacity:.85;' : '');
          sp.textContent = t;
          li.appendChild(sp);
        }
        line(x.title_en, true, false); line(x.snippet, false, true); line(x.snippet_en, true, true);
        sl.appendChild(li);
      });
      var dt = document.getElementById('diet-search-date');
      if (dt) dt.textContent = d.search.date;
    }).catch(function () {});
  }
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
