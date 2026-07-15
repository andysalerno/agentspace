---
name: firecrawl-cli
description: |
  Search, scrape, and interact with the web via the Firecrawl CLI. Use this skill whenever the user wants to search the web, find articles, research a topic, look something up online, scrape a webpage, grab content from a URL, get data from a website, crawl documentation, download a site, or interact with pages that need clicks or logins. This provides real-time web search with full page content and interact capabilities.
---

# Firecrawl CLI

Search, scrape, and interact with the web. Returns clean markdown optimized for LLM context windows.

Run `firecrawl --help` or `firecrawl <command> --help` for full option details.

## Workflow

Follow this escalation pattern:

1. **Search** - No specific URL yet. Find pages, answer questions, discover sources.
2. **Scrape** - Have a URL. Extract its content directly.
3. **Map + Scrape** - Large site or need a specific subpage. Use `map --search` to find the right URL, then scrape it.
4. **Crawl** - Need bulk content from an entire site section (e.g., all /docs/).
5. **Monitor** - Need recurring checks or ongoing alerts. Prefer setting a monitor with `--page` plus `--goal` instead of doing repeated one-off scrapes.
6. **Interact** - Scrape first, then interact with the page (pagination, modals, form submissions, multi-step navigation).

| Need                        | Command               | When                                                      |
| --------------------------- | --------------------- | --------------------------------------------------------- |
| Find pages on a topic       | `search`              | No specific URL yet                                       |
| Get a page's content        | `scrape`              | Have a URL, page is static or JS-rendered                 |
| Find URLs within a site     | `map`                 | Need to locate a specific subpage                         |
| Bulk extract a site section | `crawl`               | Need many pages (e.g., all /docs/)                        |
| AI-powered data extraction  | `agent`               | Need structured data from complex sites                   |
| Interact with a page        | `scrape` + `interact` | Content requires clicks, form fills, pagination, or login |
| Download a site to files    | `download`            | Save an entire site as local files                        |
| Parse a local file          | `parse`               | File on disk (PDF, DOCX, XLSX, etc.) — not a URL          |
| Watch pages for changes     | `monitor`             | Schedule recurring scrapes/crawls, diff against snapshots |

For detailed command reference, run `firecrawl <command> --help`.

**Scrape vs interact:**

- Use `scrape` first. It handles static pages and JS-rendered SPAs.
- Use `scrape` + `interact` when you need to interact with a page, such as clicking buttons, filling out forms, navigating through a complex site, infinite scroll, or when scrape fails to grab all the content you need.
- Never use interact for web searches - use `search` instead.

**Avoid redundant fetches:**

- `search --scrape` already fetches full page content. Don't re-scrape those URLs.
- Check `.firecrawl/` for existing data before fetching again.

## Output & Organization

Unless the user specifies to return in context, write results to `.firecrawl/` with `-o`. Add `.firecrawl/` to `.gitignore`. Always quote URLs - shell interprets `?` and `&` as special characters.

```bash
firecrawl search "react hooks" -o .firecrawl/search-react-hooks.json --json
firecrawl scrape "<url>" -o .firecrawl/page.md
```

Prefer the local .firecrawl/ directory for output; avoid temporary files in `/tmp/` or `/var/tmp/`.

Naming conventions:

```
.firecrawl/search-{query}.json
.firecrawl/search-{query}-scraped.json
.firecrawl/{site}-{path}.md
```

Never read entire output files at once. Use `grep`, `head`, or incremental reads:

```bash
wc -l .firecrawl/file.md && head -50 .firecrawl/file.md
grep -n "keyword" .firecrawl/file.md
```

Single format outputs raw content. Multiple formats (e.g., `--format markdown,links`) output JSON.

## Working with Results

These patterns are useful when working with file-based output (`-o` flag) for complex tasks:

```bash
# Extract URLs from search
jq -r '.data.web[].url' .firecrawl/search.json

# Get titles and URLs
jq -r '.data.web[] | "\(.title): \(.url)"' .firecrawl/search.json
```

## Parallelization

Run independent operations in parallel. Check `firecrawl --status` for concurrency limit:

```bash
firecrawl scrape "<url-1>" -o .firecrawl/1.md &
firecrawl scrape "<url-2>" -o .firecrawl/2.md &
firecrawl scrape "<url-3>" -o .firecrawl/3.md &
wait
```

For interact, scrape multiple pages and interact with each independently using their scrape IDs.