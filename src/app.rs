use crate::error_template::{AppError, ErrorTemplate};
use leptos::*;
use leptos_meta::*;
use leptos_router::*;
// Import on Demand
use thaw::{Button, ButtonVariant};

#[component]
pub fn App() -> impl IntoView {
    // Provides context that manages stylesheets, titles, meta tags, etc.
    provide_meta_context();

    view! {


        // injects a stylesheet into the document <head>
        // id=leptos means cargo-leptos will hot-reload this stylesheet
        <Stylesheet id="leptos" href="/pkg/leptops-axum.css"/>

        // sets the document title
        <Title text="Welcome to Leptos"/>

        // content for this welcome page
        <Router fallback=|| {
            let mut outside_errors = Errors::default();
            outside_errors.insert_with_default_key(AppError::NotFound);
            view! {
                <ErrorTemplate outside_errors/>
            }
            .into_view()
        }>
            <main>
                <Routes>
                    <Route path="" view=HomePage/>
                </Routes>
            </main>
        </Router>
    }
}

/// Renders the home page of your application.
#[component]
fn HomePage() -> impl IntoView {
    // Creates a reactive value to update the button
    let (count, set_count) = create_signal(0);
    let on_click = move |_| set_count.update(|count| *count += 1);

    view! {
        <h1>"Welcome to Leptos!zz"</h1>
        <Button variant=ButtonVariant::Primary on:click=on_click>"Click Me: " {count}</Button>
    }
}

// #[server(output = StreamingText)]
// pub async fn file_progress(filename: String) -> Result<TextStream, ServerFnError> {
//     println!("getting progress on {filename}");
//     // get the stream of current length for the file
//     let progress = progress::for_file(&filename);
//     // separate each number with a newline
//     // the HTTP response might pack multiple lines of this into a single chunk
//     // we need some way of dividing them up
//     let progress = progress.map(|bytes| Ok(format!("{bytes}\n")));
//     Ok(TextStream::new(progress))
// }
