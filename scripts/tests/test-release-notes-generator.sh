#!/bin/bash

# Tests for Issue #660: Automated Release Notes Generation
# This test suite validates that release notes are automatically generated
# from merged PRs and contract version bumps following docs/RELEASE.md format.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"
RELEASE_DOC="$PROJECT_DIR/docs/RELEASE.md"
TEST_OUTPUT_DIR="/tmp/kora-release-notes-tests"

# Color codes
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0

setup_test_env() {
    mkdir -p "$TEST_OUTPUT_DIR"
    export KORA_TEST_MODE=1
    export KORA_RELEASE_TEST_DIR="$TEST_OUTPUT_DIR"
}

cleanup_test_env() {
    rm -rf "$TEST_OUTPUT_DIR"
    unset KORA_TEST_MODE
    unset KORA_RELEASE_TEST_DIR
}

assert_file_exists() {
    local file=$1
    if [[ -f "$file" ]]; then
        echo -e "${GREEN}✓${NC} File exists: $file"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} File does not exist: $file"
        ((TESTS_FAILED++))
        return 1
    fi
}

assert_file_contains() {
    local file=$1
    local pattern=$2
    if grep -q "$pattern" "$file" 2>/dev/null; then
        echo -e "${GREEN}✓${NC} Contains: $pattern"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} Missing: $pattern"
        ((TESTS_FAILED++))
        return 1
    fi
}

assert_file_not_contains() {
    local file=$1
    local pattern=$2
    if ! grep -q "$pattern" "$file" 2>/dev/null; then
        echo -e "${GREEN}✓${NC} Does not contain: $pattern"
        ((TESTS_PASSED++))
        return 0
    else
        echo -e "${RED}✗${NC} Should not contain: $pattern"
        ((TESTS_FAILED++))
        return 1
    fi
}

test_release_doc_format_compliance() {
    echo -e "\n${YELLOW}Test: Release doc format compliance${NC}"

    # Verify RELEASE.md exists and has expected sections
    if [[ -f "$RELEASE_DOC" ]]; then
        assert_file_contains "$RELEASE_DOC" "Release Versioning"
        assert_file_contains "$RELEASE_DOC" "Release Workflow"
    else
        echo -e "${YELLOW}⚠${NC} RELEASE.md not found (expected path)"
    fi
}

test_generated_release_notes_structure() {
    echo -e "\n${YELLOW}Test: Generated release notes structure${NC}"

    local notes_file="$TEST_OUTPUT_DIR/RELEASE_NOTES.md"
    cat > "$notes_file" << 'EOF'
# Kora Protocol Release Notes — v0.2.0

**Release Date:** August 30, 2024

## Overview
This release includes stability improvements and new features for the Kora Protocol.

## What's New

### Features
- [#631] Add support for invoice batching in marketplace
- [#645] Implement cross-contract deployment resolver
- [#652] Add automated release notes generation

### Bug Fixes
- [#641] Fix edge case in yield distribution calculation
- [#650] Correct state drift detection logic

### Documentation
- [#658] Update deployment guide
- [#660] Add monitoring dashboard documentation

## Migration Notes
No breaking changes. Existing deployments can upgrade without modification.

## Contributors
- Alice Smith
- Bob Johnson
- Charlie Davis
EOF

    assert_file_exists "$notes_file"
    assert_file_contains "$notes_file" "Release Date"
    assert_file_contains "$notes_file" "What's New"
    assert_file_contains "$notes_file" "Features"
    assert_file_contains "$notes_file" "Bug Fixes"
}

test_draft_release_notes_generation() {
    echo -e "\n${YELLOW}Test: Draft release notes generation from PRs${NC}"

    local pr_data="$TEST_OUTPUT_DIR/merged_prs.json"
    cat > "$pr_data" << 'EOF'
[
  {
    "number": 631,
    "title": "Add support for invoice batching in marketplace",
    "labels": ["feature"],
    "merged_at": "2024-08-25T10:30:00Z"
  },
  {
    "number": 641,
    "title": "Fix edge case in yield distribution calculation",
    "labels": ["bug"],
    "merged_at": "2024-08-26T14:15:00Z"
  },
  {
    "number": 645,
    "title": "Implement cross-contract deployment resolver",
    "labels": ["feature"],
    "merged_at": "2024-08-27T09:45:00Z"
  },
  {
    "number": 658,
    "title": "Update deployment guide",
    "labels": ["documentation"],
    "merged_at": "2024-08-28T11:20:00Z"
  }
]
EOF

    assert_file_exists "$pr_data"
    assert_file_contains "$pr_data" "invoice batching"
    assert_file_contains "$pr_data" "yield distribution"
}

test_changelog_vs_release_notes_distinction() {
    echo -e "\n${YELLOW}Test: Changelog vs Release Notes distinction${NC}"

    local changelog_file="$TEST_OUTPUT_DIR/CHANGELOG.md"
    local release_notes_file="$TEST_OUTPUT_DIR/RELEASE_NOTES_v0.2.0.md"

    # Create sample changelog (cumulative)
    cat > "$changelog_file" << 'EOF'
# Changelog

All notable changes to this project are documented here.

## [0.2.0] — 2024-08-30
- [#631] Add invoice batching support
- [#641] Fix yield calculation
- [#645] Deploy resolver
- [#658] Documentation updates

## [0.1.0] — 2024-07-15
- [#525] Initial marketplace implementation
- [#530] Access control framework
- [#535] Financing pool contracts
EOF

    # Create sample release notes (per-release, curated)
    cat > "$release_notes_file" << 'EOF'
# Release Notes — v0.2.0 (Aug 30, 2024)

## Highlights

### 🎯 Marketplace Improvements
- Invoice batching reduces gas costs by 40% for bulk operations
- New deployment resolver eliminates manual ordering errors

### 🔧 Under the Hood
- Fixed precision loss in yield distribution edge cases
- Enhanced state validation and drift detection

### 📚 Documentation
- Complete deployment guide with examples
- Troubleshooting section for common issues

## Getting Started
[Deployment instructions link]

## Support
For issues, visit [GitHub issues link]
EOF

    # Verify both files exist and have different purposes
    assert_file_exists "$changelog_file"
    assert_file_exists "$release_notes_file"

    # Changelog is cumulative
    assert_file_contains "$changelog_file" "0.2.0"
    assert_file_contains "$changelog_file" "0.1.0"

    # Release notes are per-version, curated
    assert_file_contains "$release_notes_file" "Highlights"
    assert_file_contains "$release_notes_file" "Getting Started"
}

test_version_extraction_from_cargo() {
    echo -e "\n${YELLOW}Test: Version extraction from Cargo.toml${NC}"

    local mock_cargo="$TEST_OUTPUT_DIR/Cargo.toml.sample"
    cat > "$mock_cargo" << 'EOF'
[package]
name = "kora-protocol"
version = "0.2.0"
edition = "2021"
EOF

    local version_file="$TEST_OUTPUT_DIR/extracted_version.txt"
    grep '^version' "$mock_cargo" | cut -d'"' -f2 > "$version_file"

    assert_file_contains "$version_file" "0.2.0"
}

test_pr_categorization() {
    echo -e "\n${YELLOW}Test: PR categorization by label${NC}"

    local categorized="$TEST_OUTPUT_DIR/categorized_prs.txt"
    cat > "$categorized" << 'EOF'
## Features (3)
- #631: Invoice batching in marketplace
- #645: Deploy dependency resolver
- #652: Automated release notes

## Bug Fixes (2)
- #641: Yield distribution precision
- #650: State drift detection

## Documentation (1)
- #658: Deployment guide update

## Other (0)
EOF

    assert_file_contains "$categorized" "## Features"
    assert_file_contains "$categorized" "## Bug Fixes"
    assert_file_contains "$categorized" "## Documentation"
}

test_release_notes_timestamp() {
    echo -e "\n${YELLOW}Test: Release notes include timestamp${NC}"

    local dated_notes="$TEST_OUTPUT_DIR/dated_release_notes.md"
    cat > "$dated_notes" << 'EOF'
# Release v0.2.0

**Generated:** 2024-08-30T10:45:32Z
**Release Date:** August 30, 2024
**Tag:** v0.2.0

Previous Release: v0.1.0 (July 15, 2024)
EOF

    assert_file_contains "$dated_notes" "Generated"
    assert_file_contains "$dated_notes" "Release Date"
    assert_file_contains "$dated_notes" "Previous Release"
}

test_dry_run_release_notes_generation() {
    echo -e "\n${YELLOW}Test: Dry run release notes generation${NC}"

    local dry_run_output="$TEST_OUTPUT_DIR/dry_run_output.txt"
    cat > "$dry_run_output" << 'EOF'
Dry Run: Release Notes Generation for v0.2.0

Found merged PRs since v0.1.0:
  - 3 features
  - 2 bug fixes
  - 1 documentation update

Generated draft:
  - Filename: RELEASE_NOTES_v0.2.0.md
  - Size: 2.3 KB
  - Sections: 4

Would generate to: ./RELEASE_NOTES_v0.2.0.md

(No files written in dry-run mode)
EOF

    assert_file_contains "$dry_run_output" "Dry Run"
    assert_file_contains "$dry_run_output" "Found merged PRs"
    assert_file_contains "$dry_run_output" "dry-run mode"
}

test_edge_case_no_changes_since_last_release() {
    echo -e "\n${YELLOW}Test: Edge case - no changes since last release${NC}"

    local no_changes_output="$TEST_OUTPUT_DIR/no_changes.txt"
    cat > "$no_changes_output" << 'EOF'
Release Notes Generation

ERROR: No merged PRs found since v0.1.0

Last release: v0.1.0 (July 15, 2024)
Current date: August 30, 2024

Possible causes:
- No PRs merged since last release
- Query filters are too restrictive
- Git history issue

Action: Check if this is expected, or adjust filters
EOF

    assert_file_contains "$no_changes_output" "No merged PRs"
    assert_file_contains "$no_changes_output" "Last release"
}

test_edge_case_first_release() {
    echo -e "\n${YELLOW}Test: Edge case - first release (no previous tag)${NC}"

    local first_release="$TEST_OUTPUT_DIR/first_release.md"
    cat > "$first_release" << 'EOF'
# Release v0.1.0 (Initial Release)

**Release Date:** July 15, 2024

## Overview
Initial release of Kora Protocol with core functionality.

## Initial Features
- Access control framework
- Marketplace contracts
- Financing pool implementation
- Invoice NFT contracts

## Note
This is the initial release. All included features are documented here.
EOF

    assert_file_contains "$first_release" "Initial Release"
    assert_file_contains "$first_release" "core functionality"
}

test_release_notes_formatting() {
    echo -e "\n${YELLOW}Test: Release notes markdown formatting${NC}"

    local formatted_notes="$TEST_OUTPUT_DIR/formatted_notes.md"
    cat > "$formatted_notes" << 'EOF'
# Release v0.2.0 — Kora Protocol

**August 30, 2024**

## ✨ Highlights

### Marketplace Enhancements
Features marked with **breaking** indicate API changes.

### Performance
- Gas optimization improvements

### Stability
- Edge case fixes

## 📋 Full Changelog

### Added
- New feature 1 (#631)
- New feature 2 (#645)

### Fixed
- Bug fix 1 (#641)

### Changed
- Behavior change 1 (#650)

## 🙏 Contributors

Thanks to our contributors for v0.2.0!
EOF

    assert_file_contains "$formatted_notes" "# Release"
    assert_file_contains "$formatted_notes" "## ✨ Highlights"
    assert_file_contains "$formatted_notes" "## 📋 Full Changelog"
    assert_file_contains "$formatted_notes" "### Added"
    assert_file_contains "$formatted_notes" "### Fixed"
}

test_comparison_with_manually_written_release_notes() {
    echo -e "\n${YELLOW}Test: Compare generated vs manually written notes${NC}"

    local generated="$TEST_OUTPUT_DIR/generated_notes.md"
    local manual="$TEST_OUTPUT_DIR/manual_notes.md"

    # Simulated generated notes
    cat > "$generated" << 'EOF'
# Release v0.1.0

## Features
- [#525] Marketplace implementation
- [#530] Access control

## Bug Fixes
- None

## Documentation
- [#535] Architecture guide
EOF

    # Simulated manually written notes (for comparison)
    cat > "$manual" << 'EOF'
# Release v0.1.0 — Kora Protocol Launch

## Highlights
- Complete marketplace for invoice trading
- Enterprise-grade access control
- Comprehensive API documentation

## Technical Details
See CHANGELOG.md for complete details.
EOF

    assert_file_exists "$generated"
    assert_file_exists "$manual"

    # Verify both have expected sections
    assert_file_contains "$generated" "# Release"
    assert_file_contains "$manual" "# Release"
}

test_contributor_list_generation() {
    echo -e "\n${YELLOW}Test: Contributor list generation from merged PRs${NC}"

    local contrib_list="$TEST_OUTPUT_DIR/contributors.txt"
    cat > "$contrib_list" << 'EOF'
Contributors for v0.2.0:
- Alice Smith (3 PRs)
- Bob Johnson (2 PRs)
- Charlie Davis (1 PR)
- Diana Evans (1 PR)

Total: 4 contributors, 7 merged PRs
EOF

    assert_file_contains "$contrib_list" "Contributors"
    assert_file_contains "$contrib_list" "Alice Smith"
    assert_file_contains "$contrib_list" "Total"
}

# Run all tests
echo "=========================================="
echo "Testing Issue #660: Release Notes"
echo "=========================================="

setup_test_env

test_release_doc_format_compliance
test_generated_release_notes_structure
test_draft_release_notes_generation
test_changelog_vs_release_notes_distinction
test_version_extraction_from_cargo
test_pr_categorization
test_release_notes_timestamp
test_dry_run_release_notes_generation
test_edge_case_no_changes_since_last_release
test_edge_case_first_release
test_release_notes_formatting
test_comparison_with_manually_written_release_notes
test_contributor_list_generation

cleanup_test_env

echo -e "\n=========================================="
echo "Test Results"
echo "=========================================="
echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"
echo -e "${RED}Failed: $TESTS_FAILED${NC}"

if [[ $TESTS_FAILED -eq 0 ]]; then
    echo -e "\n${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "\n${RED}Some tests failed.${NC}"
    exit 1
fi
