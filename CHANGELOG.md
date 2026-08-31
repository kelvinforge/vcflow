# Changelog

## 0.3.0 — 2026-08-31

### Features
- feat(release): also gate Prepare Release on an idle develop
- feat(release): gate Prepare Release button on develop/production divergence

### Fixes
- fix(workflow): offer commit/finish for new work on a merged branch

### Other
- hore(ci): point macOS release build at Homebrew OpenSSL

## 0.2.2 — 2026-08-31

### Other
- chore: harden release workflow target detection and concurrency
- chore: add manual trigger and artifact upload to release workflow

## 0.2.1 — 2026-08-31

### Other
- chore: add mac + windows release CI workflow

## 0.2.0 — 2026-08-31

### Features
- feat(workflow-guard): severity styling + slugify branch names
- feat: add Workflow Guard V1 for protected branches

## 0.1.0 — 2026-08-31

### New Features

* Pull the latest changes automatically when your branch is behind.
* Added a guided Release Candidate workflow for preparing production releases.
* Automatically return to `develop` after creating a merge request.
* Added a simpler way to switch between the next action and your work list.
* Updated the app branding and now show the app version and current branch in the window title.
* Replaced the repository path field with a native folder picker.

### Improvements & Fixes

* Your work is now shown whenever there is no immediate next action.
* Added clearer guidance for making and pushing follow-up changes while a merge request is open.
* The app now detects the repository's production branch instead of assuming it is called `master`.
* Switching repositories now correctly resets the previous repository state.
* Simplified timestamps in the activity log for easier reading.

### UI Improvements

* Simplified the Start screen into a compact action row.
* Moved repository information to the top of the app for easier visibility.
* Moved repository health information to the bottom of the app.
* Added the application version to the app.

