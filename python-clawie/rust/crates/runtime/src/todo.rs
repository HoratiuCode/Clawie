#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTodo {
    pub content: String,
    pub active_form: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionTodoList {
    todos: Vec<SessionTodo>,
}

impl SessionTodoList {
    #[must_use]
    pub fn new(todos: Vec<SessionTodo>) -> Self {
        Self { todos }
    }

    #[must_use]
    pub fn todos(&self) -> &[SessionTodo] {
        &self.todos
    }

    pub fn replace(&mut self, todos: Vec<SessionTodo>) -> Result<(), String> {
        validate_todos(&todos)?;
        self.todos = if todos
            .iter()
            .all(|todo| todo.status == TodoStatus::Completed)
        {
            Vec::new()
        } else {
            todos
        };
        Ok(())
    }

    #[must_use]
    pub fn active(&self) -> Vec<&SessionTodo> {
        self.todos
            .iter()
            .filter(|todo| todo.status != TodoStatus::Completed)
            .collect()
    }
}

pub fn validate_todos(todos: &[SessionTodo]) -> Result<(), String> {
    if todos.iter().any(|todo| todo.content.trim().is_empty()) {
        return Err(String::from("todo content must not be empty"));
    }
    if todos.iter().any(|todo| todo.active_form.trim().is_empty()) {
        return Err(String::from("todo active form must not be empty"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SessionTodo, SessionTodoList, TodoStatus};

    #[test]
    fn clears_list_when_all_items_complete() {
        let mut list = SessionTodoList::default();
        list.replace(vec![SessionTodo {
            content: "Done".to_string(),
            active_form: "Finishing".to_string(),
            status: TodoStatus::Completed,
        }])
        .expect("valid todos");

        assert!(list.todos().is_empty());
    }

    #[test]
    fn preserves_parallel_in_progress_items() {
        let mut list = SessionTodoList::default();
        list.replace(vec![
            SessionTodo {
                content: "A".to_string(),
                active_form: "Doing A".to_string(),
                status: TodoStatus::InProgress,
            },
            SessionTodo {
                content: "B".to_string(),
                active_form: "Doing B".to_string(),
                status: TodoStatus::InProgress,
            },
        ])
        .expect("parallel todos are allowed");

        assert_eq!(list.active().len(), 2);
    }
}
