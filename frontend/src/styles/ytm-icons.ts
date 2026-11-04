/** YouTube Music-style icon set (SVG paths, 24px grid).
 *  Shapes follow YTM's material iconography: home, explore (compass),
 *  library, play, pause, next, prev, search, close, queue, shuffle,
 *  repeat, like (thumbs-up outline), dislike (thumbs-down outline). */

export const icons = {
  home: `<path d="M12 3l9 8h-3v9h-5v-6h-2v6H6v-9H3l9-8z"/>`,
  explore: `<path d="M12 2a10 10 0 100 20 10 10 0 000-20zm6.9 6l-2.8 6.6c-.2.5-.6.9-1.1 1.1L8.4 18.5a.6.6 0 01-.8-.8l2.8-6.6c.2-.5.6-.9 1.1-1.1l6.6-2.8a.6.6 0 01.8.8zM12 10.8a1.2 1.2 0 100 2.4 1.2 1.2 0 000-2.4z"/>`,
  library: `<path d="M4 6h2v12H4V6zm4 0h2v12H8V6zm4.2.4l7.5 2.6-.6 1.9-7.5-2.6.6-1.9zM11 20h9v2h-9v-2z"/>`,
  play: `<path d="M8 5v14l11-7L8 5z"/>`,
  pause: `<path d="M7 5h4v14H7V5zm6 0h4v14h-4V5z"/>`,
  next: `<path d="M6 6l8.5 6L6 18V6zm9 0h2.5v12H15V6z"/>`,
  prev: `<path d="M18 6l-8.5 6L18 18V6zM6 6h2.5v12H6V6z"/>`,
  search: `<path d="M15.5 14h-.8l-.3-.3a6.5 6.5 0 10-.7.7l.3.3v.8l5 5 1.5-1.5-5-5zm-6 0A4.5 4.5 0 1114 9.5 4.5 4.5 0 019.5 14z"/>`,
  close: `<path d="M18.3 5.7L12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3 1.4 1.4z" fill-rule="evenodd"/>`,
  queue: `<path d="M3 6h12v2H3V6zm0 4h12v2H3v-2zm0 4h8v2H3v-2zm14-7v8.2a2.5 2.5 0 11-2-2.45V7h2z"/>`,
  more: `<circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/>`,
  shuffle: `<path d="M17 3l4 4-4 4V9h-2.2l-2 2.3-1.4-1.3L13 8.9V7h4V3zM3 7h4.8l7.4 8.5V17h-2.1L7 10.4V7H3v0zm14 8v-2l4 4-4 4v-2h-2.6l-2-2.3 1.4-1.3 1.2 1.6H17z"/>`,
  repeat: `<path d="M7 7h10v3l4-4-4-4v3H5v6h2V7zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4z"/>`,
  like: `<path d="M18.8 8.6c-.6-2-2.4-3.6-4.4-3.6-1.6 0-2.9.9-3.6 2.2h-.1c-.2-.5-.6-1.3-1.3-1.8C8.6 4.8 7.7 4.4 6.7 4.6 5 4.9 3.8 6.4 3.8 8.2c0 2.6 1.9 4.7 5.1 7.9.7.7 1.4 1.3 2 2l1.1 1.1 1.1-1.1c3.4-3.4 6.1-6 6.1-9.2 0-.9-.2-1.8-.4-2.3zM16.8 10.9c-.5.9-1.3 1.9-2.4 3l-2.4 2.4-2.4-2.4c-2.5-2.5-4.1-4.3-4.1-5.7 0-1 .8-1.9 1.7-2 .6-.1 1.2.1 1.7.6.5.4.8 1 .8 1.6v.8h1.8v-.8c0-1 .5-2 1.4-2.5.5-.3 1.1-.4 1.7-.2 1.3.3 2.4 1.4 2.7 2.8.1.5.2 1.1.2 1.7 0 .5-.1 1-.3 1.5z"/>`,
  dislike: `<path d="M18.4 5.2c-.3-.1-.9-.2-1.4-.2h-7l-.6.2-3.2-2V12.4l3.7 3.7c.5.5 1.2.8 1.9.8h4.4c1.6 0 3-1.1 3.4-2.7l1.3-5.4c.3-1.6-.8-3.2-2.5-3.6zM19.7 9l-1.3 5.4c-.2.9-1 1.6-2 1.6h-4.4c-.4 0-.8-.2-1.1-.4l-.3-.3 4.5-4.5-1.3-1.3-5 5V5.5l2.2 1.3.6.2h7c.3 0 .7.1 1 .2.9.2 1.5 1.1 1.3 2z"/>`,
  volume: `<path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3A4.5 4.5 0 0014 8v8a4.5 4.5 0 002.5-4z"/>`,
  plus: `<path d="M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6V5z"/>`,
  eq: `<path d="M5 8h3v9H5V8zm5.5-4h3v13h-3V4zm5.5 7h3v6h-3v-6z"/>`,
  settings: `<path d="M19.4 13a7.7 7.7 0 000-2l2.1-1.6-2-3.5-2.5 1a7.6 7.6 0 00-1.7-1L15 3.2h-4l-.3 2.7a7.6 7.6 0 00-1.7 1l-2.5-1-2 3.5L6.6 11a7.7 7.7 0 000 2l-2.1 1.6 2 3.5 2.5-1a7.6 7.6 0 001.7 1l.3 2.7h4l.3-2.7a7.6 7.6 0 001.7-1l2.5 1 2-3.5L19.4 13zM13 15.5a3.5 3.5 0 110-7 3.5 3.5 0 010 7z"/>`,
  chevronLeft: `<path d="M15.4 7.4L14 6l-6 6 6 6 1.4-1.4L10.8 12z"/>`,
  chevronRight: `<path d="M8.6 16.6L10 18l6-6-6-6-1.4 1.4L13.2 12z"/>`,
} as const;

export type IconName = keyof typeof icons;

/** Inline SVG for an icon (viewBox 24, currentColor fill). */
export function icon(name: IconName, size = 24): string {
  return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="currentColor" aria-hidden="true">${icons[name]}</svg>`;
}
