# BookFinder 📚

A Rust CLI tool for searching and downloading books from multiple sources.

## Supported Sources

| Source | Search | Download | Notes |
|--------|--------|----------|-------|
| LibGen | ✅ | ✅ | Largest book library |
| Project Gutenberg | ✅ | ✅ | Free public domain books |
| Internet Archive | ✅ | ✅ | Archive.org texts collection |

## Installation

```bash
cargo install --path .
```

## Usage

### Search
```bash
# Search all sources
bookfinder search "python programming"

# Search specific source
bookfinder search -p libgen "clean code"
bookfinder search -p gutenberg "art of war"
bookfinder search -p archive "computer science"

# Limit results
bookfinder search -n 5 "algorithms"
```

### Download
```bash
# Download by provider and ID (from search results)
bookfinder download libgen <md5_hash>
bookfinder download gutenberg 132
bookfinder download archive <identifier>

# Specify output directory
bookfinder download libgen <md5> -o ./my_books/
```
