---
name: design-check
description: Screenshot the running Aurora web page and run a design-system review on it. Use when asked to check the UI/design of the current page, or after visual changes to the web shell.
---

# /design-check — live design review

Screenshot the Aurora web app as it currently renders and review it
against the design system via the `design-reviewer` agent.

## Steps

1. Ensure the dev server answers on http://127.0.0.1:1420/ (if not,
   start it: `cd frontend && npm run dev` in the background).
2. Write a playwright script to Temp (unique name, e.g.
   `aurora_design_shot.py`) that:
   - opens http://127.0.0.1:1420/#/home at 1440x900,
   - waits ~1.5s for the shell to mount,
   - saves a full-page screenshot to
     `%TEMP%\aurora-design-home.png`;
   - if a track is playing or the queue is non-empty, also screenshot
     the player bar area (element `.ytm-player`).
3. Run the script.
4. Read the screenshot(s) yourself and launch the `design-reviewer`
   agent with the screenshot path(s) and the instruction to review per
   its system prompt. Pass along the current route if it is not home.
5. Relay the agent's findings to the user, ranked; offer to apply the
   top fixes.

## Notes

- The design tokens live in `frontend/src/styles/ytm.css` (`:root`).
- Do not fix anything inside this skill — report and offer; fixes are
  a separate step the user approves.
