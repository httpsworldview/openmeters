use pipewire::spa::utils::dict::DictRef;
use std::collections::HashMap;

/// Convert a PipeWire dictionary into a convenient `HashMap` clone.
pub fn dict_to_map(dict: Option<&DictRef>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(dict) = dict {
        for (key, value) in dict.iter() {
            map.insert(key.to_string(), value.to_string());
        }
    }
    map
}
