module timing_explicit_precedence (
    input logic a,
    output logic y
);
    assign #(T_explicit, T_explicit_fall) y = a;

    specify
        (a *> y) = (T_specify, T_specify_fall);
    endspecify
endmodule
