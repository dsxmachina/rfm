# Rate Limiter Integration Refactor Plan

## Status: COMPLETE

## Goal

Integrate rate-limiting directly into DirManager and PreviewManager instead of having a separate RateLimiter component. This eliminates extra channels and spawned tasks.

## Architecture Change

### Before
```
Panel → input_channel → RateLimiter → manager_channel → DirManager/PreviewManager
```

### After
```
Panel → channel → DirManager/PreviewManager (with integrated rate limiting)
```

## Benefits

- Eliminates 2 extra unbounded channels
- Eliminates 2 spawned rate limiter tasks
- Simpler data flow
- Related logic stays together

---

## Tasks

- [x] Task 1: Add rate-limit state to manager structs
- [x] Task 2: Add rate-limit methods to managers
- [x] Task 3: Change manager loops to use tokio::select!
- [x] Task 4: Update main.rs to remove rate limiter
- [x] Task 5: Delete rate_limiter.rs module
- [x] Task 6: Move unit tests to content.rs
- [x] Task 7: Update config handling
- [x] Task 8: Final verification

## Commit

```
cafe213 refactor(rate-limiter): integrate rate limiting into managers
```

## Verification

- 13 tests pass
- Clippy: 0 errors
- Release build: success
