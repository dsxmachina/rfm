## Testing the Rate Limiter

### Manual Test Procedure

1. Start the application: `cargo run`
2. Navigate to a directory with many files
3. Open a terminal in that directory
4. Run a command that creates many files rapidly:
   ```bash
   for i in {1..100}; do touch "testfile_$i.txt"; done
   ```
5. Observe the panel - it should update smoothly every ~500ms, not 100 times
6. Clean up: `rm testfile_*.txt`

### Verify with Debug Logging

Add temporary debug output to rate_limiter.rs:
```rust
if send_now {
    eprintln!("[RATE] Allowing request for {:?}", key);
} else if schedule_delayed {
    eprintln!("[RATE] Scheduling delayed for {:?}", key);
} else {
    eprintln!("[RATE] Dropping request for {:?}", key);
}
```

Run with: `cargo run 2> /tmp/rate-limit.log`
Then check the log to see rate limiting in action.
