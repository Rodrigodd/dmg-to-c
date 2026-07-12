module timing_ambiguous_paths (
    input logic a, b,
    output logic y
);
    assign y = a | b;

    specify
        (a *> y) = (T_first, T_first_fall);
        (b *> y) = (T_second, T_second_fall);
    endspecify
endmodule
