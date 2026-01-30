use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TopicJson {
    pub id: u64,
    pub title: String,
    pub post_stream: PostStream,
}

#[derive(Debug, Deserialize)]
pub struct PostStream {
    pub posts: Vec<Post>,
}

#[derive(Debug, Deserialize)]
pub struct Post {
    pub post_number: u64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_username: Option<String>,
    #[serde(default)]
    pub avatar_template: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub cooked: Option<String>,
}
