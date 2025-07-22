(function() {
    var implementors = Object.fromEntries([["sui_http",[["impl&lt;B&gt; Body for <a class=\"struct\" href=\"sui_http/middleware/grpc_timeout/struct.MaybeEmptyBody.html\" title=\"struct sui_http::middleware::grpc_timeout::MaybeEmptyBody\">MaybeEmptyBody</a>&lt;B&gt;<div class=\"where\">where\n    B: Body + <a class=\"trait\" href=\"https://doc.rust-lang.org/1.88.0/core/marker/trait.Send.html\" title=\"trait core::marker::Send\">Send</a>,</div>"],["impl&lt;B, ResponseHandlerT&gt; Body for <a class=\"struct\" href=\"sui_http/middleware/callback/struct.ResponseBody.html\" title=\"struct sui_http::middleware::callback::ResponseBody\">ResponseBody</a>&lt;B, ResponseHandlerT&gt;<div class=\"where\">where\n    B: Body,\n    B::Error: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.88.0/core/fmt/trait.Display.html\" title=\"trait core::fmt::Display\">Display</a> + 'static,\n    ResponseHandlerT: <a class=\"trait\" href=\"sui_http/middleware/callback/trait.ResponseHandler.html\" title=\"trait sui_http::middleware::callback::ResponseHandler\">ResponseHandler</a>,</div>"]]]]);
    if (window.register_implementors) {
        window.register_implementors(implementors);
    } else {
        window.pending_implementors = implementors;
    }
})()
//{"start":57,"fragment_lengths":[1041]}