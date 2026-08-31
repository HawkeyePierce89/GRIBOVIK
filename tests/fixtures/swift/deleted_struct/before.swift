struct Legacy {
    let id: Int
}

extension Legacy {
    func describe() -> String {
        return "Legacy(\(id))"
    }
}

func keep() -> Legacy {
    return Legacy(id: 1)
}
