# Literature System Architecture

Three-layer Zettelkasten design for literature management, separating vendor-managed highlights from personal knowledge.

## The Problem

Vendor-managed files (Readwise exports, Zotero annotations) were polluting the knowledge graph. Hundreds of auto-generated files contained wiki links, creating false connections in the backlink system and inflating graph metrics. The knowledge base couldn't distinguish between genuine intellectual connections and vendor-generated references.

## The Solution: Three-Layer Architecture

### Layer 1: Vendor Systems (Outside Knowledge Base)

Highlights and annotations stay in their vendor systems. They are not part of the knowledge base filesystem.

| Source | Storage | Purpose |
|--------|---------|---------|
| Zotero | Zotero database | PDF annotations, citation metadata |
| Readwise | `~/Captures/readwise/` | Kindle highlights, web highlights, article saves |
| Kindle | Synced to Readwise | Book highlights |

These are **raw material**, not knowledge. They exist outside the knowledge base directory.

### Layer 2: Processing Notes (Pyramids)

Source-bound thinking notes live in a dedicated subdirectory within the knowledge base:

```
~/notes/LIT/
├── Kahneman-Thinking-Fast-Slow.md
├── Ahrens-How-To-Take-Smart-Notes.md
└── ...
```

**Pyramids** are notes that remain bound to their source. They contain:
- Key arguments and evidence from the source
- Your reactions and questions
- Connections to other sources
- Page/location references

Pyramids can contain wiki links to other knowledge base notes, but they are clearly marked as source-dependent thinking.

### Layer 3: Permanent Notes (Spheres)

Source-independent insights live at the root of the knowledge base:

```
~/notes/
├── Cognitive-Bias.md
├── Zettelkasten-Method.md
├── Second-Order-Thinking.md
└── ...
```

**Spheres** are notes that stand on their own. They express an idea without requiring knowledge of any particular source. A sphere might have been sparked by reading Kahneman, but the note itself is about cognitive bias in general.

### The Pyramid-to-Sphere Pipeline

```
[Vendor Highlights] → [Processing Notes (Pyramids)] → [Permanent Notes (Spheres)]
                       Source-bound thinking            Source-independent insight
```

Not every pyramid produces a sphere. Some sources inform existing spheres rather than creating new ones. The pipeline is:

1. **Read** and highlight in vendor tool (Kindle, browser, PDF)
2. **Review** highlights and write pyramid note (source-bound reactions)
3. **Extract** insights that stand independently → create or update sphere notes
4. **Link** spheres to each other (not to vendor files)

## Infrastructure Layer

Supporting tools live outside the knowledge base:

```
~/Literature/
├── library.bib          # BibTeX citation database
├── search-indexes/      # Semantic search embeddings
└── readwise-archive/    # Historical export snapshots
```

These are infrastructure, not knowledge. They support the workflow but don't participate in the knowledge graph.

## Workflows

### Academic Reading (Zotero)

1. Import paper to Zotero, annotate PDF
2. Export annotations via Zotero integration
3. Write pyramid note in `~/notes/LIT/Author-Title.md`
4. Extract insights to sphere notes
5. Cite using `fcit` (citation picker)

### General Reading (Readwise)

1. Highlight in Kindle/browser/article
2. Readwise syncs to `~/Captures/readwise/`
3. Review highlights, write pyramid note
4. Extract insights to sphere notes

### Key Principle

The knowledge base contains **your thinking**, not **other people's words**. Vendor systems hold the raw material; the knowledge base holds what you've made of it.

## Design Decisions

**Why not keep Readwise exports in the knowledge base?**
- They generate hundreds of files with wiki links
- Backlink counts become meaningless (most links are vendor-generated)
- Graph analysis shows false clusters around popular sources
- Search results get cluttered with highlight fragments

**Why separate pyramids from spheres?**
- Pyramids are useful for revisiting a source
- Spheres are useful for thinking about a topic
- The same idea can appear in both, expressed differently
- Separation makes the knowledge base navigable by topic, not by source

**Why a `LIT/` subdirectory?**
- Pyramids are numerous and source-bound
- Keeping them in a subdirectory prevents root-level clutter
- They still participate in the knowledge graph via wiki links
- Easy to exclude from certain searches when needed
