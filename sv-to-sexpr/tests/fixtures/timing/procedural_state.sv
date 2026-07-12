module timing_procedural_state (
    input logic ena, a, b,
    output logic q
);
    initial q = 0;
    always_latch if (ena) q <= a & b;

    specify
        (ena *> q) = (T_state, T_state_fall);
    endspecify
endmodule
