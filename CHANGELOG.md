# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- The GitHub Release job now declares needs: [prepare, publish]; v0.5.3's release step still failed with 'requires a tag' because needs.prepare.outputs.tag was out of scope.

## [0.5.3] - 2026-10-25
### Fixed
- The GitHub release step now receives the resolved tag name; v0.5.2 published to crates.io but its GitHub release step failed with 'requires a tag' (main-push trigger has no ref_name tag).

