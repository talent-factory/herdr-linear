//! GraphQL query and mutation definitions

/// Get authenticated viewer (current user)
pub const QUERY_VIEWER: &str = r#"
query Viewer {
  viewer {
    id
    email
    name
    avatarUrl
    createdAt
    updatedAt
  }
}
"#;

/// Get teams accessible to the user
pub const QUERY_TEAMS: &str = r#"
query Teams($first: Int, $after: String) {
  teams(first: $first, after: $after) {
    nodes {
      id
      key
      name
      description
      avatarUrl
      createdAt
      updatedAt
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
      startCursor
      endCursor
    }
    totalCount
  }
}
"#;

/// Get team by ID
pub const QUERY_TEAM: &str = r#"
query Team($id: String!) {
  team(id: $id) {
    id
    key
    name
    description
    avatarUrl
    createdAt
    updatedAt
  }
}
"#;

/// Get issues for a team with optional filters
pub const QUERY_ISSUES: &str = r#"
query Issues(
  $first: Int,
  $after: String,
  $filter: IssueFilter,
  $orderBy: IssueOrderByInput
) {
  issues(
    first: $first,
    after: $after,
    filter: $filter,
    orderBy: $orderBy
  ) {
    nodes {
      id
      identifier
      title
      description
      priority
      estimate
      url
      createdAt
      updatedAt
      startedAt
      completedAt
      state {
        id
        name
        type
      }
      team {
        id
        key
        name
        description
        avatarUrl
        createdAt
        updatedAt
      }
      assignee {
        id
        email
        name
        avatarUrl
        createdAt
        updatedAt
      }
      creator {
        id
        email
        name
        avatarUrl
        createdAt
        updatedAt
      }
      cycle {
        id
        number
        title
        createdAt
        updatedAt
        startedAt
        endsAt
        completedAt
      }
      project {
        id
        name
        description
        url
        state
        createdAt
        updatedAt
        startDate
        targetDate
      }
      labels(first: 50) {
        nodes {
          id
          name
          color
          description
          createdAt
          updatedAt
        }
      }
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
      startCursor
      endCursor
    }
    totalCount
  }
}
"#;

/// Get single issue by ID
pub const QUERY_ISSUE: &str = r#"
query Issue($id: String!) {
  issue(id: $id) {
    id
    identifier
    title
    description
    priority
    estimate
    url
    createdAt
    updatedAt
    startedAt
    completedAt
    state {
      id
      name
      type
    }
    team {
      id
      key
      name
      description
      avatarUrl
      createdAt
      updatedAt
    }
    assignee {
      id
      email
      name
      avatarUrl
      createdAt
      updatedAt
    }
    creator {
      id
      email
      name
      avatarUrl
      createdAt
      updatedAt
    }
    cycle {
      id
      number
      title
      createdAt
      updatedAt
      startedAt
      endsAt
      completedAt
    }
    project {
      id
      name
      description
      url
      state
      createdAt
      updatedAt
      startDate
      targetDate
    }
    labels(first: 50) {
      nodes {
        id
        name
        color
        description
        createdAt
        updatedAt
      }
    }
    comments(first: 50) {
      nodes {
        id
        body
        user {
          id
          email
          name
          avatarUrl
          createdAt
          updatedAt
        }
        createdAt
        updatedAt
      }
    }
  }
}
"#;

/// Create a new issue
pub const MUTATION_CREATE_ISSUE: &str = r#"
mutation CreateIssue($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue {
      id
      identifier
      title
      description
      priority
      url
      createdAt
      updatedAt
      state {
        id
        name
        type
      }
      team {
        id
        key
        name
      }
    }
  }
}
"#;

/// Update an issue
pub const MUTATION_UPDATE_ISSUE: &str = r#"
mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue {
      id
      identifier
      title
      description
      priority
      estimate
      state {
        id
        name
        type
      }
      assignee {
        id
        name
        email
      }
    }
  }
}
"#;

/// Add a comment to an issue
pub const MUTATION_ADD_COMMENT: &str = r#"
mutation AddComment($input: CommentCreateInput!) {
  commentCreate(input: $input) {
    success
    comment {
      id
      body
      user {
        id
        name
        email
      }
      createdAt
    }
  }
}
"#;

/// Get all projects
pub const QUERY_PROJECTS: &str = r#"
query Projects($first: Int, $after: String, $filter: ProjectFilter) {
  projects(first: $first, after: $after, filter: $filter) {
    nodes {
      id
      name
      description
      url
      state
      createdAt
      updatedAt
      startDate
      targetDate
      lead {
        id
        name
        email
      }
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
      startCursor
      endCursor
    }
    totalCount
  }
}
"#;

/// Get cycles for a team
pub const QUERY_CYCLES: &str = r#"
query Cycles($teamId: String!, $first: Int, $after: String) {
  cycles(teamId: $teamId, first: $first, after: $after) {
    nodes {
      id
      number
      title
      startedAt
      endsAt
      completedAt
      createdAt
      updatedAt
      team {
        id
        key
        name
      }
    }
    pageInfo {
      hasNextPage
      hasPreviousPage
      startCursor
      endCursor
    }
    totalCount
  }
}
"#;

/// Get workflow states for a team
pub const QUERY_WORKFLOW_STATES: &str = r#"
query WorkflowStates($teamId: String!, $first: Int) {
  workflowStates(teamId: $teamId, first: $first) {
    nodes {
      id
      name
      type
      position
    }
  }
}
"#;
