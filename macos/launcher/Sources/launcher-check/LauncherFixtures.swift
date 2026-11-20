import Foundation
import LauncherCore

// The renderer and the checks decode identical CLI documents.
enum LauncherFixtures {
    static func apps() throws -> AppList {
        let rows = [
            #"{"id":"ubuntu/firefox","name":"Firefox","machine":"Ubuntu","host":"ubuntu","extra":1}"#, // check-private: ok (invented machine label)
            #"{"id":"ubuntu/blender","name":"Blender","machine":"Ubuntu","host":"ubuntu"}"#, // check-private: ok (invented machine label)
            #"{"id":"ubuntu/editor","name":"Text Editor","machine":"Ubuntu","host":"ubuntu"}"#, // check-private: ok (invented machine label)
            #"{"id":"ubuntu/chrome","name":"Chrome","machine":"Ubuntu","host":"ubuntu"}"#, // check-private: ok (invented machine label)
            #"{"id":"mac/TextEdit","name":"TextEdit","machine":"Studio","host":"mac"}"#, // check-private: ok (invented machine label)
            #"{"id":"local/com.example.calculator","name":"Calculator","machine":"This Mac","host":"local"}"#,
            #"{"id":"local/com.example.calendar","name":"Calendar","machine":"This Mac","host":"local"}"#,
        ]
        let json = #"{"apps":[\#(rows.joined(separator: ","))],"problems":[]}"#
        return try JSONDecoder().decode(AppList.self, from: Data(json.utf8))
    }

    static func projects() throws -> [ProjectRow] {
        let rows = [
            #"{"id":"0123456789abcdef0123456789abcdef","name":"alpha","kinds":["rust"],"development_machine":"Ubuntu","reachable":true,"clash":false,"capabilities":{"gpu":true,"tools":["cargo"]},"extra":true}"#, // check-private: ok (invented machine label)
            #"{"id":"00000000000000000000000000000002","name":"scene","kinds":["blender"],"development_machine":"Ubuntu","reachable":false,"clash":false,"capabilities":{"gpu":true,"tools":["blender"]}}"#, // check-private: ok (invented machine label)
            #"{"id":"00000000000000000000000000000003","name":"notes-app","kinds":["apple"],"development_machine":"Studio","reachable":true,"clash":false,"capabilities":{"gpu":false,"tools":[]}}"#, // check-private: ok (invented machine label)
            #"{"id":"00000000000000000000000000000004","name":"shared","kinds":["unknown"],"development_machine":"Ubuntu","reachable":true,"clash":true,"capabilities":{"gpu":false,"tools":[]}}"#, // check-private: ok (invented machine label)
            #"{"id":"00000000000000000000000000000004","name":"shared","kinds":["unknown"],"development_machine":"Example A","reachable":false,"clash":true,"capabilities":{"gpu":false,"tools":[]}}"#, // check-private: ok (invented machine label)
        ]
        let json = "[" + rows.joined(separator: ",") + "]"
        return try JSONDecoder().decode([ProjectRow].self, from: Data(json.utf8))
    }
}
