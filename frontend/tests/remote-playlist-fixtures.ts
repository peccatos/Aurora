import type { StoredTrack } from "../src/library-db";
import type {
  RemotePlaylistSnapshot,
  RemotePlaylistTrack,
  RemoteProvider,
} from "../src/remote-playlist-import";

/** Fixtures shared by the remote-playlist-import suites. */

let seq = 0;

export function localTrack(
  id: string,
  name: string,
  artist: string | null,
  overrides: Partial<StoredTrack> = {},
): StoredTrack {
  return {
    id,
    name,
    artist,
    album: null,
    durationMs: null,
    extension: "mp3",
    bytes: new ArrayBuffer(0),
    addedAt: ++seq,
    ...overrides,
  };
}

export function remoteTrack(
  remoteId: string,
  title: string,
  artists: string[],
  position: number,
  overrides: Partial<RemotePlaylistTrack> = {},
): RemotePlaylistTrack {
  return { remoteId, title, artists, album: null, durationMs: null, position, ...overrides };
}

export function snapshot(
  tracks: RemotePlaylistTrack[],
  overrides: Partial<RemotePlaylistSnapshot> = {},
): RemotePlaylistSnapshot {
  return {
    provider: "spotify" as RemoteProvider,
    id: "playlist-1",
    name: "Road trip",
    trackCount: tracks.length,
    revision: "rev-1",
    tracks,
    ...overrides,
  };
}

/** Records the calls the import makes on the local playlist store. */
export function playlistRecorder(): {
  created: { id: string; name: string }[];
  added: { playlistId: string; trackIds: string[] }[];
  createPlaylist: (name: string) => Promise<{ id: string; name: string; trackIds: string[]; createdAt: number }>;
  addToPlaylist: (playlistId: string, trackIds: string[]) => Promise<void>;
} {
  const created: { id: string; name: string }[] = [];
  const added: { playlistId: string; trackIds: string[] }[] = [];
  return {
    created,
    added,
    createPlaylist: async (name: string) => {
      const playlist = { id: `local-${created.length + 1}`, name, trackIds: [], createdAt: 1 };
      created.push({ id: playlist.id, name: playlist.name });
      return playlist;
    },
    addToPlaylist: async (playlistId: string, trackIds: string[]) => {
      added.push({ playlistId, trackIds: [...trackIds] });
    },
  };
}
