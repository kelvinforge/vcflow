import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/** Normalise free text into a branch slug for a live input: lowercase, runs of
 *  non-alphanumerics collapsed to a single hyphen, leading hyphens trimmed. A
 *  trailing hyphen is kept so the user can keep typing the next word; the
 *  backend `git_core::slugify` trims it and stays the authority. */
export function slugify(raw: string): string {
  return raw
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+/, "")
}
