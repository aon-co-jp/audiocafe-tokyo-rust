#!/usr/bin/env python3
"""毎朝、ダイエットレシピの一覧ページ3つを巡回し、先頭のレシピが変わったら「最新」と「一つ前」を更新する。
他社ページの写しは保存しない(レシピの題名とURLだけを記録)。"""
import html, json, os, re, sys, time, urllib.request
from datetime import datetime, timedelta, timezone

UA = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120 Safari/537.36"
STATE = os.environ.get("DIET_STATE", "/root/diet-recipes/state.json")
OUTS = os.environ.get("DIET_OUTS", "/root/aruaru.tokyo/static/diet-recipes.json,/var/www/audiocafe.tokyo/diet-recipes.json").split(",")
SOURCES = [
    ("kikkoman", "https://www.kikkoman.co.jp/homecook/theme/popular/healthy-200calories-recipes.html", r'href="(/homecook/search/recipe/\d+/)"'),
    ("nadia", "https://oceans-nadia.com/recipe_list/5060", r'href="(/user/\d+/recipe/\d+)"'),
    ("rakuten", "https://recipe.rakuten.co.jp/category/39-504/", r'href="(/recipe/\d+/)"'),
]
JST = timezone(timedelta(hours=9))


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept-Language": "ja"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read().decode("utf-8", "replace")


def title_of(page):
    m = re.search(r"<title[^>]*>([^<]*)</title>", page)
    t = html.unescape(m.group(1)).strip() if m else ""
    t = re.split(r"\s+[|｜]\s+|\s+-\s+(?=楽天レシピ|Nadia)", t)[0].strip()
    return t


def main():
    try:
        state = json.load(open(STATE, encoding="utf-8"))
    except Exception:
        state = {}
    today = datetime.now(JST).strftime("%Y-%m-%d")
    changed = False
    for key, page_url, rx in SOURCES:
        try:
            listing = get(page_url)
            m = re.search(rx, listing)
            if not m:
                print(key, "no recipe link found", file=sys.stderr)
                continue
            base = re.match(r"https?://[^/]+", page_url).group(0)
            url = base + m.group(1)
            st = state.setdefault(key, {"page_url": page_url, "page_title": title_of(listing), "latest": None, "previous": None})
            st["page_title"] = title_of(listing) or st.get("page_title", "")
            if st["latest"] and st["latest"]["url"] == url:
                continue
            title = title_of(get(url)) or url
            if st["latest"]:
                st["previous"] = st["latest"]
            st["latest"] = {"title": title, "url": url, "date": today}
            changed = True
            print(key, "updated:", title, url)
            time.sleep(2)
        except Exception as e:
            print(key, "error:", e, file=sys.stderr)
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    json.dump(state, open(STATE, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    for out in OUTS:
        if os.path.isdir(os.path.dirname(out)):
            json.dump({"updated": today, "sources": state}, open(out, "w", encoding="utf-8"), ensure_ascii=False)
    print("done changed=%s" % changed)


main()
