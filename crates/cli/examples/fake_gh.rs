//! Stands in for `gh` in the tests: GitHub's issues, pull requests and comments
//! as one JSON file (`CONVOY_FAKE_GH`), the code in the bare repo it names.

mod fake;

fn main() {
    let st = fake::State::open("CONVOY_FAKE_GH");
    let line = st.argv.join(" ");
    let injected = |key: &str| {
        st.doc[key]
            .as_array()
            .is_some_and(|parts| parts.iter().filter_map(|p| p.as_str()).any(|p| line.contains(p)))
    };
    if injected("fail") {
        st.finish("", 1, "fake gh: injected failure\n");
    }
    st.finish("", 1, &format!("fake gh: unsupported call {:?}\n", st.argv));
}
