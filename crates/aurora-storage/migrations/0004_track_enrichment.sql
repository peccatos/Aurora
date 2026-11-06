-- Track enrichment: user-supplied and provider-sourced metadata.
-- Every value keeps its source so the UI can distinguish local edits from
-- provider enrichment and avoid silently overwriting user data.
ALTER TABLE tracks ADD COLUMN lyrics TEXT;
ALTER TABLE tracks ADD COLUMN lyrics_language TEXT;
ALTER TABLE tracks ADD COLUMN lyrics_translation TEXT;
ALTER TABLE tracks ADD COLUMN lyrics_translation_language TEXT;
ALTER TABLE tracks ADD COLUMN artwork_data_url TEXT;
ALTER TABLE tracks ADD COLUMN source_url TEXT;
ALTER TABLE tracks ADD COLUMN source_provider TEXT;
ALTER TABLE tracks ADD COLUMN video_url TEXT;
ALTER TABLE tracks ADD COLUMN metadata_source TEXT;

CREATE INDEX idx_tracks_source ON tracks (source_provider, source_url);
