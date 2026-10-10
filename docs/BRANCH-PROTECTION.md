# Branch protection for `main`

A ready-to-apply guide for the maintainer. Nothing here is applied yet.

## 1. Required status check

Require **`checks`**, the job in `.github/workflows/ci.yml`. Its steps run inside that one job, so they are not separate required checks.

## 2. Reviews

- Require a pull request before merging, with **1** approving review.
- Require review from Code Owners. `.github/CODEOWNERS` assigns `*` to `@erictan2001`.

## 3. Force pushes and deletions

- Block force pushes (`non_fast_forward`).
- Block deletion of `main` (`deletion`).

## 4. Up to date before merge

- Require branches to be up to date before merging (`strict_required_status_checks_policy: true`).

## 5. Decide before applying: the release bot's push

Pushes made with `GITHUB_TOKEN` do not trigger workflows; once protection is on, that push is rejected unless its actor can bypass the rules (`docs/BACKLOG.md`, "Process", records the same concern). Choose one:

- **A. Add the `github-actions` bot as a bypass actor.** Smaller change; the bump commit skips review and checks.
- **B. Switch the push to a GitHub App token** and add that App as the bypass actor. Its pushes trigger workflows, so the bump gets CI.

Until one is in place, `sync-main-version` fails at its push and prints `npm run sync:version -- vX` as the manual repair.

## 6. Applying it

### Option 1: GitHub web UI

1. **Settings > Rules > Rulesets > New branch ruleset.** Name it `protect-main`, set Enforcement to **Active**, and target the default branch.
2. Enable **Restrict deletions** and **Block force pushes**.
3. Enable **Require a pull request before merging**: Required approvals `1`, plus **Require review from Code Owners**.
4. Enable **Require status checks to pass**: add `checks`. Enable **Require branches to be up to date before merging**.
5. Under **Bypass list**, add what you chose in section 5, then create the ruleset.

### Option 2: `gh api` (UNEXECUTED TEMPLATE)

```bash
# UNEXECUTED TEMPLATE. Save the JSON below as protect-main.json, fill "bypass_actors"
# per section 5 (actor IDs are not verified here), then run:
#    gh api --method POST repos/erictan2001/floaty/rulesets --input protect-main.json
```

```json
{
  "name": "protect-main",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/heads/main"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    {
      "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 1,
        "require_code_owner_review": true,
        "dismiss_stale_reviews_on_push": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false
      }
    },
    {
      "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": true,
        "required_status_checks": [{ "context": "checks" }]
      }
    }
  ]
}
```

## 7. Verifying it

1. Confirm the ruleset is active: `gh api repos/erictan2001/floaty/rulesets`
2. Open a test PR with a trivial docs change. Check that:
   - the `checks` job runs and merge waits for it;
   - merge stays blocked until one approval from `@erictan2001`;
   - after another PR lands on `main`, the test PR must update its branch before merging.
3. From a non-bypass account, check that `git push origin HEAD:main` and a force push to `main` are both rejected.
4. At the next release, confirm `sync-main-version` succeeds or fails loudly, never silently. Then close the test PR.
