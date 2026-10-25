# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- The GitHub release step now receives the resolved tag name; v0.5.2 published to crates.io but its GitHub release step failed with 'requires a tag' (main-push trigger has no ref_name tag).

## [0.5.2] - 2026-10-25
### Changed
- The release workflow follows the repo release standard: a `chore(release): vX.Y.Z` commit on main triggers tag creation (release App) and the crates.io publish + GitHub release.

