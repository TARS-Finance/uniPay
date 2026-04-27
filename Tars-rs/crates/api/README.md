# API

A standardized REST API primitives library for uniPay Rust applications.

## Overview

This crate provides consistent, typed response structures for REST APIs in the uniPay ecosystem. It ensures that all API endpoints return responses with a predictable format, making client integration simpler and more reliable.

## Usage

The `api` crate is designed to be used as a sub-crate within the uniPay package:

```rust
use unipay::api::primitives::{Response, Status};
```

## Features

### Standardized Response Structure

Every API response follows a consistent format with status indication and appropriate result or error fields:

```rust
use unipay::api::primitives::{Response, Status};
use axum::Json;

// Success response with data
let success_response: Json<Response<String>> = Response::ok("User created successfully".to_string());

// Error response
let error_response = Response::<()>::error("Invalid input parameters");
```

### Response Format

All responses serialize to JSON with this structure:

```json
// Success example
{
  "status": "Ok",
  "result": "User created successfully"
}

// Error example
{
  "status": "Error",
  "error": "Invalid input parameters"
}
```

### Integration with Axum

The API primitives integrate seamlessly with the Axum web framework:

```rust
use unipay::api::primitives::Response;
use axum::{Router, routing::get};

async fn get_user(id: String) -> Response<User> {
    match db.find_user(&id).await {
        Ok(user) => Response::ok(user),
        Err(e) => Response::error(format!("Failed to find user: {}", e))
    }
}

let app = Router::new()
    .route("/users/:id", get(get_user));
```

### Type-Safe Responses

The `Response<T>` type is generic over the result payload type, providing type safety:

```rust
use unipay::api::primitives::Response;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
struct UserProfile {
    id: String,
    name: String,
    email: String,
}

// Type-safe response with UserProfile data
fn get_profile(user_id: &str) -> Response<UserProfile> {
    // Implementation...
    # Response::ok(UserProfile { id: user_id.to_string(), name: "Example User".to_string(), email: "user@example.com".to_string() })
}

// The compiler ensures we're returning the correct type
let profile_response: Response<UserProfile> = get_profile("user123");
```

## Error Handling

The API primitives make it easy to convert various error types into standardized responses:

```rust
use unipay::api::primitives::Response;
use axum::extract::Path;

async fn handler(Path(id): Path<String>) -> Json<Response<User>> {
    match fetch_user(&id).await {
        Ok(user) => Response::ok(user),
        Err(DatabaseError::NotFound) => Response::error("User not found"),
        Err(DatabaseError::ConnectionError(e)) => Response::error(format!("Database error: {}", e)),
        Err(_) => Response::error("An unexpected error occurred")
    }
}
```