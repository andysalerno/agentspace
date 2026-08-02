/*
 * Tabster's modalizer marks non-active surfaces `aria-hidden` from an async
 * pass. Under jsdom that pass can run before a freshly opened surface has been
 * registered, so the surface hides itself and its contents disappear from role
 * queries. Browsers order this correctly; tests opt into hidden elements when
 * querying inside a dialog rather than depending on the race.
 */
export const IN_DIALOG = { hidden: true } as const;
