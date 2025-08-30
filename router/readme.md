>[!NOTE]
> Enidu's Router is how you control enidu instances. Each instance has an emergency STOP / Panic button. 



GET /api/v1
```

Returns information about the API status and version.

**Response:**

```json
{
  "status": "200",
  "message": "Enidu Stress Test API v1",
  "version": "2.1.0"
}
```

#### Create a Stress Test Job

```
POST /api/v1/jobs
```

Starts a new stress test against a target URL.

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| url | string | **Required**. The target URL to stress test. |
| proxyAddr | string | Optional. Proxy server to route requests through. |
| concurrency | int | Number of concurrent connections (default: 100). |
| timeoutSec | int | Request timeout in seconds (default: 10). |
| waitMs | int | Milliseconds to wait between requests (default: 0). |
| randomHeaders | bool | Whether to use randomized HTTP headers (default: false). |

**Example Request:**

```json
{
  "url": "https://example.com/api",
  "concurrency": 500,
  "timeoutSec": 15,
  "waitMs": 50,
  "randomHeaders": true
}
```

**Example Response:**

```json
{
  "id": "f8e5a2b7c6d9",
  "wsUrl": "/api/v1/ws/f8e5a2b7c6d9",
  "status": "running",
  "info": {
    "id": "f8e5a2b7c6d9",
    "url": "https://example.com/api",
    "concurrency": 500,
    "timeoutSec": 15,
    "waitMs": 50,
    "randomHeaders": true,
    "startTime": "2023-08-23T12:34:56.789Z",
    "status": "running"
  }
}
```

#### Get Job Status

```
GET /api/v1/jobs/:id
```

Returns the current status and statistics for a job.

**Example Response:**

```json
{
  "id": "f8e5a2b7c6d9",
  "url": "https://example.com/api",
  "concurrency": 500,
  "timeoutSec": 15,
  "waitMs": 50,
  "randomHeaders": true,
  "startTime": "2023-08-23T12:34:56.789Z",
  "status": "running",
  "stats": {
    "sent": 15000,
    "success": 14500,
    "errors": 500,
    "timeouts": 200,
    "connErrors": 300,
    "updateTime": "2023-08-23T12:35:56.789Z"
  }
}
```

#### List All Jobs

```
GET /api/v1/jobs
```

Returns a list of all jobs, including active and recently completed jobs.

**Example Response:**

```json
{
  "count": 2,
  "jobs": [
    {
      "id": "f8e5a2b7c6d9",
      "url": "https://example.com/api",
      "concurrency": 500,
      "timeoutSec": 15,
      "waitMs": 50,
      "randomHeaders": true,
      "startTime": "2023-08-23T12:34:56.789Z",
      "status": "running",
      "stats": {
        "sent": 15000,
        "success": 14500,
        "errors": 500,
        "timeouts": 200,
        "connErrors": 300,
        "updateTime": "2023-08-23T12:35:56.789Z"
      }
    },
    {
      "id": "a1b2c3d4e5f6",
      "url": "https://api.example.org",
      "concurrency": 200,
      "timeoutSec": 5,
      "waitMs": 0,
      "randomHeaders": false,
      "startTime": "2023-08-23T12:30:00.000Z",
      "status": "complete",
      "stats": {
        "sent": 50000,
        "success": 48000,
        "errors": 2000,
        "timeouts": 500,
        "connErrors": 1500,
        "updateTime": "2023-08-23T12:33:00.000Z"
      }
    }
  ]
}
```

#### Stop a Job

```
DELETE /api/v1/jobs/:id
```

Stops a running stress test job.

**Example Response:**

```json
{
  "status": "stopping",
  "id": "f8e5a2b7c6d9"
}
```

#### Stop All Jobs

```
DELETE /api/v1/jobs
```

Stops all running stress test jobs.

**Example Response:**

```json
{
  "status": "stopping_all",
  "count": 3
}
```

### WebSocket API

#### Stream Job Logs

```
WebSocket /api/v1/ws/:id
```

Connect to this WebSocket endpoint to receive real-time logs and updates from a running job.

**Messages:**

The server will send JSON messages of different types:

1. **Info Message** - Sent when the connection is established:

```json
{
  "type": "info",
  "data": {
    "id": "f8e5a2b7c6d9",
    "url": "https://example.com/api",
    "concurrency": 500,
    "timeoutSec": 15,
    "waitMs": 50,
    "randomHeaders": true,
    "startTime": "2023-08-23T12:34:56.789Z",
    "status": "running"
  }
}
```

2. **Log Message** - Sent for each log line from the stress tester:

```json
{
  "type": "log",
  "data": "Runtime: 5.2s | Reqs: 2500 (RPS: 500 | Avg: 481.2) | Success: 2400 (480/s) | Errors: 100 (20/s) | Timeouts: 50 | ConnErrs: 50",
  "ts": 1692788123
}
```

## Examples

### cURL Examples

#### Create a Stress Test Job

```bash
curl -X POST http://localhost:3171/api/v1/jobs \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://example.com",
    "concurrency": 200,
    "timeoutSec": 5,
    "randomHeaders": true
  }'
```

#### Stop a Job

```bash
curl -X DELETE http://localhost:3171/api/v1/jobs/f8e5a2b7c6d9
```

### JavaScript Example

```javascript
// Create a stress test job
async function createStressTest() {
  const response = await fetch('http://localhost:3171/api/v1/jobs', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json'
    },
    body: JSON.stringify({
      url: 'https://example.com',
      concurrency: 200,
      timeoutSec: 5,
      randomHeaders: true
    })
  });
  
  const data = await response.json();
  console.log('Job created:', data);
  
  // Connect to WebSocket for logs
  const ws = new WebSocket(`ws://localhost:3171${data.wsUrl}`);
  
  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'log') {
      console.log(message.data);
    } else if (message.type === 'info') {
      console.log('Job info:', message.data);
    }
  };
  
  return data.id;
}

// Stop a stress test job
async function stopStressTest(id) {
  const response = await fetch(`http://localhost:3171/api/v1/jobs/${id}`, {
    method: 'DELETE'
  });
  
  const data = await response.json();
  console.log('Job stopped:', data);
}
```

## Error Codes

| Code | Description |
|------|-------------|
| MISSING_URL | The target URL parameter is missing |
| JOB_NOT_FOUND | The specified job ID was not found |
| INVALID_JOB_ID | The WebSocket connection was attempted with an invalid job ID |

## Rate Limits

There are currently no rate limits imposed on the API.
This api is made to handle over 100 requests at the same time. 
```



