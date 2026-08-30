# Advanced IPC Deep Dive for Otto

## Purpose

This document provides technical details on advanced IPC mechanisms that could be useful for future Otto features like:
- Real-time task progress updates
- Streaming data pipelines
- Live task coordination
- Large dataset transfers

This is **NOT** a proposal for immediate implementation, but rather a research document for future reference.

## 1. Memory-Mapped Files (mmap)

### What Is It?

Memory-mapped files allow processes to access file contents as if they were in memory, with the OS handling page faults and synchronization.

### How It Works

```
Process A                    Kernel                      Process B
   |                            |                            |
   | mmap(fd, ...)             |                            |
   |--------------------------->|                            |
   | Returns: addr1            |                            |
   |                            |                            |
   |                            |   mmap(fd, ...)           |
   |                            |<---------------------------|
   |                            | Returns: addr2            |
   |                            |                            |
   | write to addr1[0]          |                            |
   |--------------------------->|                            |
   |                            | Page updated              |
   |                            | (shared memory)           |
   |                            |                            |
   |                            |   read from addr2[0]      |
   |                            |<---------------------------|
   |                            | Returns: same data        |
```

### Implementation Options

#### Option A: POSIX Shared Memory

```rust
// In Otto (Rust)
use std::fs::OpenOptions;
use memmap2::MmapMut;

pub struct TaskDataRegion {
    file: File,
    mmap: MmapMut,
}

impl TaskDataRegion {
    pub fn create(task_name: &str, size: usize) -> Result<Self> {
        let path = format!("/dev/shm/otto-{}-{}", std::process::id(), task_name);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;

        file.set_len(size as u64)?;

        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self { file, mmap })
    }

    pub fn write_json(&mut self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_vec(data)?;
        if json.len() > self.mmap.len() {
            return Err(eyre!("Data too large for mmap region"));
        }

        // Write length header (4 bytes)
        self.mmap[0..4].copy_from_slice(&(json.len() as u32).to_le_bytes());

        // Write JSON data
        self.mmap[4..4+json.len()].copy_from_slice(&json);

        // Flush to ensure visibility
        self.mmap.flush()?;

        Ok(())
    }

    pub fn read_json(&self) -> Result<serde_json::Value> {
        // Read length header
        let len = u32::from_le_bytes([
            self.mmap[0], self.mmap[1], self.mmap[2], self.mmap[3]
        ]) as usize;

        if len == 0 || len > self.mmap.len() - 4 {
            return Ok(serde_json::Value::Null);
        }

        // Read JSON data
        let data = &self.mmap[4..4+len];
        Ok(serde_json::from_slice(data)?)
    }
}
```

**Bash Access (requires helper binary):**
```bash
# Otto would provide a helper tool
otto-mmap-write task_a key value
value=$(otto-mmap-read task_a key)
```

**Python Access:**
```python
import mmap
import json
import os

def read_task_data(task_name):
    path = f"/dev/shm/otto-{os.getppid()}-{task_name}"
    with open(path, 'r+b') as f:
        mm = mmap.mmap(f.fileno(), 0)
        length = int.from_bytes(mm[0:4], 'little')
        if length > 0:
            data = mm[4:4+length]
            return json.loads(data)
    return None

def write_task_data(task_name, data):
    path = f"/dev/shm/otto-{os.getppid()}-{task_name}"
    json_data = json.dumps(data).encode('utf-8')
    with open(path, 'r+b') as f:
        mm = mmap.mmap(f.fileno(), 0)
        mm[0:4] = len(json_data).to_bytes(4, 'little')
        mm[4:4+len(json_data)] = json_data
        mm.flush()
```

#### Option B: Anonymous Shared Memory

```rust
use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};

pub struct SharedMemory {
    ptr: *mut u8,
    size: usize,
}

impl SharedMemory {
    pub fn new(size: usize) -> Result<Self> {
        let ptr = unsafe {
            mmap(
                None,
                size.try_into()?,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED | MapFlags::MAP_ANONYMOUS,
                -1,
                0,
            )?
        };

        Ok(Self {
            ptr: ptr as *mut u8,
            size,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        unsafe {
            munmap(self.ptr as *mut _, self.size).ok();
        }
    }
}
```

### Pros and Cons

**Pros:**
- ✅ Extremely fast (memory speed, no syscalls after setup)
- ✅ Zero-copy data sharing
- ✅ OS handles synchronization and caching
- ✅ Can use file-backed or anonymous memory
- ✅ Survives process crashes (file-backed)

**Cons:**
- ❌ Fixed size (or complex resizing logic needed)
- ❌ Requires synchronization primitives for safe concurrent access
- ❌ Platform-specific implementations
- ❌ Difficult to use from bash without helper tools
- ❌ Cleanup on error is tricky
- ❌ Not human-readable (binary format)

### When To Use

- Large datasets (> 100MB) that would be slow to serialize/deserialize
- Real-time updates (task progress, streaming metrics)
- Circular buffers (logging, event streams)
- High-frequency updates (> 100 updates/sec)

### When NOT To Use

- Small data (< 1KB) - overhead not worth it
- Infrequent updates (< 1/min) - files are fine
- When debuggability matters - binary format hard to inspect
- Cross-platform requirements - implementation varies

## 2. Unix Domain Sockets

### What Is It?

Unix domain sockets (UDS) are IPC sockets that use filesystem paths instead of network addresses. They provide reliable, bidirectional, byte-stream communication.

### How It Works

```
Otto Process (Server)         Kernel                Task Process (Client)
       |                         |                          |
       | socket(AF_UNIX)        |                          |
       | bind(/tmp/otto.sock)   |                          |
       | listen()               |                          |
       |----------------------->|                          |
       | Listening...           |                          |
       |                         |   socket(AF_UNIX)       |
       |                         |   connect(/tmp/otto.sock)|
       |                         |<-------------------------|
       | accept() -> client_fd  |                          |
       |<------------------------|                          |
       |                         |                          |
       | read(client_fd)        |   write(sock, "key=val") |
       |<----------------------------------------------------|
       |                         |                          |
       | write(client_fd, "OK") |   read(sock) -> "OK"     |
       |---------------------------------------------------->|
```

### Implementation Example

#### Otto Server (Rust)

```rust
use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct OttoDataServer {
    socket_path: PathBuf,
    listener: UnixListener,
    data_store: Arc<Mutex<HashMap<String, serde_json::Value>>>,
}

impl OttoDataServer {
    pub async fn new(run_id: u64) -> Result<Self> {
        let socket_path = PathBuf::from(format!("/tmp/otto-{}.sock", run_id));

        // Remove old socket if exists
        let _ = tokio::fs::remove_file(&socket_path).await;

        let listener = UnixListener::bind(&socket_path)?;

        Ok(Self {
            socket_path,
            listener,
            data_store: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn run(&self) -> Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let data_store = self.data_store.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_client(stream, data_store).await {
                    eprintln!("Client error: {}", e);
                }
            });
        }
    }

    async fn handle_client(
        mut stream: UnixStream,
        data_store: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    ) -> Result<()> {
        let mut buffer = vec![0u8; 4096];

        loop {
            let n = stream.read(&mut buffer).await?;
            if n == 0 {
                break; // Connection closed
            }

            let message = String::from_utf8_lossy(&buffer[..n]);
            let response = Self::handle_message(&message, &data_store)?;

            stream.write_all(response.as_bytes()).await?;
        }

        Ok(())
    }

    fn handle_message(
        message: &str,
        data_store: &Arc<Mutex<HashMap<String, serde_json::Value>>>,
    ) -> Result<String> {
        let parts: Vec<&str> = message.trim().splitn(3, ' ').collect();

        match parts.as_slice() {
            ["SET", key, value] => {
                let json_value: serde_json::Value = serde_json::from_str(value)?;
                data_store.lock().unwrap().insert(key.to_string(), json_value);
                Ok("OK\n".to_string())
            }
            ["GET", key] => {
                let value = data_store.lock().unwrap()
                    .get(*key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                Ok(format!("{}\n", serde_json::to_string(&value)?))
            }
            ["LIST"] => {
                let keys: Vec<String> = data_store.lock().unwrap()
                    .keys()
                    .cloned()
                    .collect();
                Ok(format!("{}\n", serde_json::to_string(&keys)?))
            }
            _ => Ok("ERROR: Invalid command\n".to_string()),
        }
    }
}
```

#### Task Client (Bash)

```bash
#!/bin/bash

# Helper function to communicate with Otto socket
otto_socket_send() {
    local cmd="$1"
    echo "$cmd" | nc -U "$OTTO_SOCKET" || echo "ERROR"
}

# Set data
otto_set() {
    local key="$1"
    local value="$2"
    otto_socket_send "SET $OTTO_TASK_NAME.$key \"$value\""
}

# Get data
otto_get() {
    local key="$1"
    otto_socket_send "GET $key" | jq -r .
}

# Example usage
otto_set "version" "1.2.3"
otto_set "status" "building"

# Get from another task
build_version=$(otto_get "build.version")
```

#### Task Client (Python)

```python
import socket
import json
import os

class OttoClient:
    def __init__(self):
        self.socket_path = os.environ.get('OTTO_SOCKET')
        self.task_name = os.environ.get('OTTO_TASK_NAME')

    def _send(self, command):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.connect(self.socket_path)
            sock.sendall(command.encode('utf-8'))
            response = sock.recv(4096).decode('utf-8')
            return response.strip()

    def set(self, key, value):
        json_value = json.dumps(value)
        cmd = f"SET {self.task_name}.{key} {json_value}"
        return self._send(cmd) == "OK"

    def get(self, key):
        cmd = f"GET {key}"
        response = self._send(cmd)
        return json.loads(response)

    def list_keys(self):
        cmd = "LIST"
        response = self._send(cmd)
        return json.loads(response)

# Usage
otto = OttoClient()
otto.set("version", "1.2.3")
otto.set("status", "building")

build_version = otto.get("build.version")
```

### Protocol Design

**Simple Text Protocol:**
```
Commands:
  SET <key> <json_value>     - Store a value
  GET <key>                  - Retrieve a value
  LIST                       - List all keys
  DELETE <key>               - Delete a value
  SUBSCRIBE <pattern>        - Subscribe to key changes (pub/sub)

Responses:
  OK                         - Success
  <json_value>               - For GET commands
  ERROR: <message>           - Error occurred
```

**Binary Protocol (More Efficient):**
```
Message Format:
  [4 bytes: message length]
  [1 byte: command type]
  [N bytes: payload]

Command Types:
  0x01 = SET
  0x02 = GET
  0x03 = DELETE
  0x04 = LIST
  0x05 = SUBSCRIBE

Response Format:
  [4 bytes: response length]
  [1 byte: status code]
  [N bytes: payload]

Status Codes:
  0x00 = OK
  0x01 = Error
  0x02 = Not Found
```

### Pros and Cons

**Pros:**
- ✅ Bidirectional communication
- ✅ Connection-oriented (reliable delivery)
- ✅ Can stream data (no size limits)
- ✅ Standard POSIX API
- ✅ Automatic cleanup on process death
- ✅ Can implement request/response or pub/sub
- ✅ Better than TCP (no network overhead)

**Cons:**
- ❌ Requires Otto to run a server thread
- ❌ More complex than files
- ❌ Protocol design needed
- ❌ Bash/Python need socket client code
- ❌ Data not persistent (in-memory only)
- ❌ Can't inspect data easily (no files to cat)

### When To Use

- Real-time task coordination
- Progress updates during long-running tasks
- Event notifications (pub/sub pattern)
- Task-to-task messaging
- Cancellation signals
- Live debugging/inspection

### When NOT To Use

- Simple data passing (files are better)
- One-time data transfer at task end
- When debugging matters (ephemeral data)
- Batch/offline processing

## 3. Named Pipes (FIFOs)

### What Is It?

Named pipes are filesystem objects that act as unidirectional byte streams between processes.

### Implementation Example

#### Otto Setup (Rust)

```rust
use std::fs;
use std::os::unix::fs::FileTypeExt;

pub struct TaskPipe {
    path: PathBuf,
}

impl TaskPipe {
    pub fn create(task_name: &str, direction: &str) -> Result<Self> {
        let path = PathBuf::from(format!("/tmp/otto-{}-{}.fifo", task_name, direction));

        // Remove old pipe if exists
        if path.exists() {
            fs::remove_file(&path)?;
        }

        // Create FIFO
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR)?;

        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TaskPipe {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
```

#### Task Usage (Bash)

```bash
# Producer task
echo '{"key": "value", "count": 42}' > "$OTTO_OUTPUT_PIPE"
echo '{"another": "item"}' > "$OTTO_OUTPUT_PIPE"
# Pipe stays open, can write multiple times

# Consumer task
while read -r line; do
    echo "Received: $line"
    # Process JSON
    key=$(echo "$line" | jq -r .key)
    count=$(echo "$line" | jq -r .count)
done < "$OTTO_INPUT_PIPE"
```

#### Task Usage (Python)

```python
import json
import os

# Producer
with open(os.environ['OTTO_OUTPUT_PIPE'], 'w') as pipe:
    for i in range(100):
        data = {"index": i, "value": f"item-{i}"}
        json.dump(data, pipe)
        pipe.write('\n')  # Newline delimiter
        pipe.flush()  # Important!

# Consumer
with open(os.environ['OTTO_INPUT_PIPE'], 'r') as pipe:
    for line in pipe:
        data = json.loads(line)
        print(f"Processing: {data}")
```

### Streaming Architecture

```yaml
tasks:
  generate_data:
    streaming: true
    bash: |
      # Generate 1000 items, one per second
      for i in {1..1000}; do
        echo "{\"id\": $i, \"timestamp\": \"$(date -Iseconds)\"}"
        sleep 1
      done > "$OTTO_OUTPUT_PIPE"

  process_data:
    before: [generate_data]
    streaming: true
    bash: |
      # Process items as they arrive (no waiting!)
      while read -r item; do
        id=$(echo "$item" | jq -r .id)
        echo "Processing item $id..."
        # Do work...
      done < "$OTTO_INPUT_PIPE"

  aggregate:
    before: [process_data]
    streaming: true
    python3: |
      import json
      import sys

      count = 0
      with open(os.environ['OTTO_INPUT_PIPE'], 'r') as pipe:
          for line in pipe:
              count += 1
              if count % 100 == 0:
                  print(f"Processed {count} items so far...", file=sys.stderr)

      print(f"Total: {count} items")
```

### Pros and Cons

**Pros:**
- ✅ True streaming (no buffering)
- ✅ Natural Unix primitive
- ✅ Automatic synchronization (blocking)
- ✅ No size limits
- ✅ Simple API (just file I/O)
- ✅ Works with shell redirects

**Cons:**
- ❌ Unidirectional (need pairs for bidirectional)
- ❌ Blocking can cause deadlocks
- ❌ Data is ephemeral (no replay)
- ❌ Can't inspect contents (no cat on pipe)
- ❌ Requires careful EOF handling

### When To Use

- Log streaming and processing
- Data transformation pipelines
- Real-time event processing
- Large datasets (don't fit in memory)
- Producer/consumer patterns

## 4. Comparison Matrix

| Feature | Files | mmap | UDS | FIFOs |
|---------|-------|------|-----|-------|
| **Speed** | Slow | Very Fast | Fast | Fast |
| **Streaming** | No | Partial | Yes | Yes |
| **Persistent** | Yes | Yes | No | No |
| **Debuggable** | Easy | Hard | Hard | Hard |
| **Bash-Friendly** | Easy | Hard | Medium | Easy |
| **Bidirectional** | Yes | Yes | Yes | No |
| **Setup Complexity** | Low | High | High | Medium |
| **Runtime Overhead** | Medium | Low | Low | Low |
| **Size Limits** | No | Yes | No | No |
| **Cross-Platform** | Excellent | Fair | Good | Good |

## 5. Recommendations for Otto

### Current: Keep Files

**For standard data passing between tasks:**
- ✅ Keep the current file-based approach
- ✅ It works, it's debuggable, it's simple
- ✅ Focus on improving the API (see improved-data-passing-proposal.md)

### Future: Add Streaming Option

**For streaming workflows (Phase 2):**
- Consider **Named Pipes (FIFOs)**
- Add `streaming: true` flag to tasks
- Set up FIFO pairs automatically
- Document patterns and gotchas

**Example Future API:**
```yaml
tasks:
  tail_logs:
    streaming: true
    bash: |
      tail -f /var/log/app.log > "$OTTO_OUTPUT_PIPE"

  parse_logs:
    before: [tail_logs]
    streaming: true
    python3: |
      import re
      for line in sys.stdin:  # Connected to tail_logs pipe
          if re.search(r'ERROR', line):
              print(f"Found error: {line}")
```

### Future: Live Progress (Phase 3)

**For real-time task monitoring:**
- Consider **Unix Domain Sockets**
- Otto runs server during execution
- Tasks can send progress updates
- TUI displays live updates

**Example Future API:**
```bash
# In task script
for i in {1..100}; do
    otto-progress $i 100 "Processing item $i"
    # Do work...
done
```

## Conclusion

Advanced IPC methods have their place, but not for basic task data passing. The current file-based approach is the right choice for Otto's core use case.

Consider advanced IPC only when:
1. **Streaming is essential** (pipes/FIFOs)
2. **Real-time coordination needed** (sockets)
3. **Huge datasets** (mmap for zero-copy)

For 95% of Otto use cases, improved file-based API (see improved-data-passing-proposal.md) is the best solution.
