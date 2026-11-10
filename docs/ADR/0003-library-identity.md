# ADR 0003: Library identity and storage

Status: Accepted

## Context

Phase 6 introduces the track library: folder scanning, metadata extraction,
playlists and search. The library must survive rescans, file moves and
interrupted scans without duplicating entries, and must display correct
track names (tags when present, file stem otherwise).

## Decision

1. **Track identity is content-derived.** A track's `TrackId` is folded
   from the first 16 bytes of the blake3 digest of the file's content
   (blake3 already in-tree, no new cryptographic dependency). The same
   file on a rescan resolves to the same id, so inserts use
   `ON CONFLICT(id) DO UPDATE` — a rescan refreshes path and metadata in
   place, never duplicates (Phase 6 exit criterion).

2. **SQLite, WAL, one migration per schema change.** All SQL lives in
   `aurora-storage` (invariant 6). WAL + `synchronous=NORMAL`: the library
   tolerates losing the last transaction on a power cut; the next scan
   repairs it. Every schema change is a migration file.

3. **Correct names are a domain rule, not a UI patch.** `Track::display_title()`
   returns the tagged title or the file stem (never the full path, never a
   placeholder); `display_line()` joins "Artist — Title". The tauri DTOs and
   queue entries carry these strings so every surface shows the same name.

4. **Untagged files are first-class.** Missing tags are `None`, not errors.
   Only unreadable/corrupt files are counted as scan failures (logged,
   skipped; a rescan completes the job — interruption recovery).

5. **ReplayGain tags flow from library to DSP.** The metadata layer parses
   them (accepting `-6.5 dB` / `-6.5` forms, rejecting garbage and absurd
   values); `Track` carries track/album dB; playback passes them into the
   pipeline, which applies them only when the user enables ReplayGain.
   Honest reporting: an active ReplayGain is a `Gain` transformation in
   ADR 0002 diagnostics.

6. **Scan serialisation.** `LibraryService` guards scans with an async
   mutex: a rescan racing a scan could prune freshly-added tracks. Metadata
   reads run via `spawn_blocking`; hashing and SQL stay off the UI thread.

## Consequences

- Two identical files in different folders are one library entry (same
  content): the path points to the most recently scanned copy. Acceptable:
  identity is what the user means by "the same recording".
- Pruning is path-based: a moved file looks like delete+add (new content
  identity stays the same, path upserts — actually a move is an upsert,
  only true deletions prune).
- The `.db` path is chosen by the user via dialog; auto-open at a default
  app-data location is a Phase 7 product decision.
