-- Phase 11: measured tempo for DJ decks.
--
-- bpm is a MEASURED property of the content (decoded and analyzed), not a
-- tag echo. NULL = not yet measured; the analyzer fills it on first deck
-- load / analysis pass. Never fabricated by a rescan.
ALTER TABLE tracks ADD COLUMN bpm REAL;
